//! Declarative planning for nested offscreen composition.
//!
//! This module deliberately does not execute Flux or Prism operations. It
//! compiles image-producing passes into a DAG, derives image lifetimes for
//! target reuse, and plans the regions each pass must read and rewrite for a
//! frame. The compositor remains responsible for mapping a pass to a concrete
//! raster, compute, copy, or material operation.

use std::collections::{HashMap, HashSet, VecDeque};
use std::error::Error;
use std::fmt;

/// Integer image-local rectangle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub const fn is_empty(self) -> bool {
        self.width <= 0 || self.height <= 0
    }

    pub fn intersect(self, other: Self) -> Option<Self> {
        let x0 = self.x.max(other.x);
        let y0 = self.y.max(other.y);
        let x1 = self
            .x
            .saturating_add(self.width)
            .min(other.x.saturating_add(other.width));
        let y1 = self
            .y
            .saturating_add(self.height)
            .min(other.y.saturating_add(other.height));
        (x1 > x0 && y1 > y0)
            .then(|| Self::new(x0, y0, x1.saturating_sub(x0), y1.saturating_sub(y0)))
    }

    pub fn union(self, other: Self) -> Self {
        if self.is_empty() {
            return other;
        }
        if other.is_empty() {
            return self;
        }
        let x0 = self.x.min(other.x);
        let y0 = self.y.min(other.y);
        let x1 = self
            .x
            .saturating_add(self.width)
            .max(other.x.saturating_add(other.width));
        let y1 = self
            .y
            .saturating_add(self.height)
            .max(other.y.saturating_add(other.height));
        Self::new(x0, y0, x1.saturating_sub(x0), y1.saturating_sub(y0))
    }

    fn subtract(self, hole: Self) -> Vec<Self> {
        if self.is_empty() || hole.is_empty() {
            return vec![self];
        }
        let Some(clip) = self.intersect(hole) else {
            return vec![self];
        };
        let right = self.x.saturating_add(self.width);
        let bottom = self.y.saturating_add(self.height);
        let clip_right = clip.x.saturating_add(clip.width);
        let clip_bottom = clip.y.saturating_add(clip.height);
        let mut result = Vec::with_capacity(4);
        if clip.y > self.y {
            result.push(Self::new(
                self.x,
                self.y,
                self.width,
                clip.y.saturating_sub(self.y),
            ));
        }
        if clip_bottom < bottom {
            result.push(Self::new(
                self.x,
                clip_bottom,
                self.width,
                bottom.saturating_sub(clip_bottom),
            ));
        }
        if clip.x > self.x {
            result.push(Self::new(
                self.x,
                clip.y,
                clip.x.saturating_sub(self.x),
                clip.height,
            ));
        }
        if clip_right < right {
            result.push(Self::new(
                clip_right,
                clip.y,
                right.saturating_sub(clip_right),
                clip.height,
            ));
        }
        result
    }
}

/// Stable index of an image-producing node inside one composition graph.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(usize);

impl NodeId {
    pub fn index(self) -> usize {
        self.0
    }
}

/// Physical target selected by lifetime analysis.
///
/// Nodes with the same target may reuse one allocation, but never during
/// overlapping lifetimes. External sources have no target assignment.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TargetId(usize);

impl TargetId {
    pub fn index(self) -> usize {
        self.0
    }
}

/// Extent and caller-defined compatibility class of an image.
///
/// The graph treats `class` as opaque. Callers should include every property
/// that prevents target aliasing, such as format, sample count, color space,
/// and usage flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ImageDesc {
    pub width: u32,
    pub height: u32,
    pub class: u64,
}

impl ImageDesc {
    pub const fn new(width: u32, height: u32, class: u64) -> Self {
        Self {
            width,
            height,
            class,
        }
    }

    pub fn bounds(self) -> Rect {
        Rect::new(
            0,
            0,
            i32::try_from(self.width).unwrap_or(i32::MAX),
            i32::try_from(self.height).unwrap_or(i32::MAX),
        )
    }

    fn is_valid(self) -> bool {
        self.width > 0
            && self.height > 0
            && self.width <= i32::MAX as u32
            && self.height <= i32::MAX as u32
    }
}

/// Storage lifetime requested for a pass output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Storage {
    /// Contents only need to survive until the last consumer in this frame.
    /// Compatible, non-overlapping outputs may share a physical target.
    Transient,
    /// Contents may be reused by a later frame. The output receives a unique
    /// target and frame planning consults its validity before skipping work.
    Persistent,
}

/// Per-edge expansion around a mapped region.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Padding {
    pub left: u32,
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
}

impl Padding {
    pub const fn uniform(value: u32) -> Self {
        Self {
            left: value,
            top: value,
            right: value,
            bottom: value,
        }
    }
}

/// Conservative coordinate relationship between one input and a pass output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegionMap {
    /// Coordinates are mapped between the input and output extents. This is
    /// identity when the extents match and a nearest enclosing scale when they
    /// differ. `sample` expands reverse ROI; `damage` expands the forward
    /// response footprint.
    Scale { sample: Padding, damage: Padding },
    /// Any requested output may read the complete input and any input change
    /// may affect the complete output. Use for operations without a tighter
    /// conservative mapping.
    Full,
}

impl RegionMap {
    pub const fn identity() -> Self {
        Self::Scale {
            sample: Padding::uniform(0),
            damage: Padding::uniform(0),
        }
    }

    pub const fn local(sample_radius: u32, damage_radius: u32) -> Self {
        Self::Scale {
            sample: Padding::uniform(sample_radius),
            damage: Padding::uniform(damage_radius),
        }
    }

    fn required_input(
        self,
        output: &RegionSet,
        input_desc: ImageDesc,
        output_desc: ImageDesc,
        limit: usize,
    ) -> RegionSet {
        match self {
            Self::Full => {
                if output.is_empty() {
                    RegionSet::new()
                } else {
                    RegionSet::full(input_desc)
                }
            }
            Self::Scale { sample, .. } => {
                output.map_scaled(output_desc, input_desc, sample, input_desc.bounds(), limit)
            }
        }
    }

    fn damaged_output(
        self,
        input: &RegionSet,
        input_desc: ImageDesc,
        output_desc: ImageDesc,
        output_domain: Rect,
        limit: usize,
    ) -> RegionSet {
        match self {
            Self::Full => {
                if input.is_empty() {
                    RegionSet::new()
                } else {
                    RegionSet::from_rect(output_domain)
                }
            }
            Self::Scale { damage, .. } => {
                input.map_scaled(input_desc, output_desc, damage, output_domain, limit)
            }
        }
    }
}

/// A bounded set of image-local rectangles.
///
/// Rectangles are kept disjoint. If propagation would exceed the graph's
/// region budget, the set conservatively collapses to one bounding rectangle.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RegionSet {
    rects: Vec<Rect>,
}

impl RegionSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_rect(rect: Rect) -> Self {
        if rect.is_empty() {
            Self::new()
        } else {
            Self { rects: vec![rect] }
        }
    }

    pub fn from_rects(rects: impl IntoIterator<Item = Rect>) -> Self {
        let mut result = Self::new();
        for rect in rects {
            result.add(rect, usize::MAX);
        }
        result
    }

    pub fn full(desc: ImageDesc) -> Self {
        Self::from_rect(desc.bounds())
    }

    pub fn as_slice(&self) -> &[Rect] {
        &self.rects
    }

    pub fn is_empty(&self) -> bool {
        self.rects.is_empty()
    }

    pub fn bounding_rect(&self) -> Option<Rect> {
        self.rects.iter().copied().reduce(Rect::union)
    }

    fn add(&mut self, rect: Rect, limit: usize) {
        if rect.is_empty() {
            return;
        }
        let mut uncovered = vec![rect];
        for existing in &self.rects {
            uncovered = uncovered
                .into_iter()
                .flat_map(|piece| piece.subtract(*existing))
                .collect();
            if uncovered.is_empty() {
                return;
            }
        }
        self.rects.extend(uncovered);
        self.enforce_limit(limit);
    }

    fn union_with(&mut self, other: &Self, limit: usize) {
        for rect in &other.rects {
            self.add(*rect, limit);
        }
    }

    fn intersect(&self, clip: Rect, limit: usize) -> Self {
        let mut result = Self::new();
        for rect in &self.rects {
            if let Some(rect) = rect.intersect(clip) {
                result.add(rect, limit);
            }
        }
        result
    }

    fn enforce_limit(&mut self, limit: usize) {
        if self.rects.len() > limit {
            self.rects = self.bounding_rect().into_iter().collect();
        }
    }

    fn map_scaled(
        &self,
        from: ImageDesc,
        to: ImageDesc,
        padding: Padding,
        clip: Rect,
        limit: usize,
    ) -> Self {
        let mut result = Self::new();
        for rect in &self.rects {
            let x0 =
                scale_floor(rect.x, from.width, to.width).saturating_sub(u32_to_i32(padding.left));
            let y0 =
                scale_floor(rect.y, from.height, to.height).saturating_sub(u32_to_i32(padding.top));
            let x1 = scale_ceil(rect.x.saturating_add(rect.width), from.width, to.width)
                .saturating_add(u32_to_i32(padding.right));
            let y1 = scale_ceil(rect.y.saturating_add(rect.height), from.height, to.height)
                .saturating_add(u32_to_i32(padding.bottom));
            let mapped = Rect::new(x0, y0, x1.saturating_sub(x0), y1.saturating_sub(y0));
            if let Some(mapped) = mapped.intersect(clip) {
                result.add(mapped, limit);
            }
        }
        result
    }
}

fn u32_to_i32(value: u32) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

fn scale_floor(value: i32, from: u32, to: u32) -> i32 {
    if value <= 0 {
        return 0;
    }
    let scaled = i64::from(value)
        .saturating_mul(i64::from(to))
        .div_euclid(i64::from(from));
    i32::try_from(scaled).unwrap_or(i32::MAX)
}

fn scale_ceil(value: i32, from: u32, to: u32) -> i32 {
    if value <= 0 {
        return 0;
    }
    let numerator = i64::from(value)
        .saturating_mul(i64::from(to))
        .saturating_add(i64::from(from) - 1);
    i32::try_from(numerator.div_euclid(i64::from(from))).unwrap_or(i32::MAX)
}

#[derive(Clone, Debug)]
struct Dependency {
    source: NodeId,
    map: RegionMap,
}

#[derive(Clone, Debug)]
enum NodeKind {
    Source,
    Pass {
        storage: Storage,
        dependencies: Vec<Dependency>,
    },
}

#[derive(Clone, Debug)]
struct Node {
    label: String,
    image: ImageDesc,
    domain: Rect,
    kind: NodeKind,
}

/// Hard limits applied while compiling and planning a graph.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GraphLimits {
    pub max_nodes: usize,
    pub max_edges: usize,
    pub max_regions_per_node: usize,
}

impl Default for GraphLimits {
    fn default() -> Self {
        Self {
            max_nodes: 256,
            max_edges: 512,
            max_regions_per_node: 64,
        }
    }
}

/// Mutable declaration of image sources, passes, and their dependencies.
#[derive(Clone, Debug)]
pub struct CompositionGraph {
    nodes: Vec<Node>,
    limits: GraphLimits,
}

impl Default for CompositionGraph {
    fn default() -> Self {
        Self::new(GraphLimits::default())
    }
}

impl CompositionGraph {
    pub fn new(limits: GraphLimits) -> Self {
        Self {
            nodes: Vec::new(),
            limits,
        }
    }

    pub fn add_source(
        &mut self,
        label: impl Into<String>,
        image: ImageDesc,
    ) -> Result<NodeId, GraphError> {
        self.add_node(label, image, image.bounds(), NodeKind::Source)
    }

    pub fn add_pass(
        &mut self,
        label: impl Into<String>,
        image: ImageDesc,
        domain: Rect,
        storage: Storage,
    ) -> Result<NodeId, GraphError> {
        self.add_node(
            label,
            image,
            domain,
            NodeKind::Pass {
                storage,
                dependencies: Vec::new(),
            },
        )
    }

    fn add_node(
        &mut self,
        label: impl Into<String>,
        image: ImageDesc,
        domain: Rect,
        kind: NodeKind,
    ) -> Result<NodeId, GraphError> {
        if self.nodes.len() >= self.limits.max_nodes {
            return Err(GraphError::NodeLimit {
                limit: self.limits.max_nodes,
            });
        }
        if !image.is_valid() {
            return Err(GraphError::InvalidImage(image));
        }
        let Some(domain) = domain.intersect(image.bounds()) else {
            return Err(GraphError::EmptyDomain);
        };
        let id = NodeId(self.nodes.len());
        self.nodes.push(Node {
            label: label.into(),
            image,
            domain,
            kind,
        });
        Ok(id)
    }

    pub fn add_dependency(
        &mut self,
        pass: NodeId,
        source: NodeId,
        map: RegionMap,
    ) -> Result<(), GraphError> {
        if self.edge_count() >= self.limits.max_edges {
            return Err(GraphError::EdgeLimit {
                limit: self.limits.max_edges,
            });
        }
        if source.0 >= self.nodes.len() {
            return Err(GraphError::UnknownNode(source));
        }
        let Some(node) = self.nodes.get_mut(pass.0) else {
            return Err(GraphError::UnknownNode(pass));
        };
        let NodeKind::Pass { dependencies, .. } = &mut node.kind else {
            return Err(GraphError::SourceHasDependencies(pass));
        };
        if dependencies.iter().any(|edge| edge.source == source) {
            return Err(GraphError::DuplicateDependency { pass, source });
        }
        dependencies.push(Dependency { source, map });
        Ok(())
    }

    fn edge_count(&self) -> usize {
        self.nodes
            .iter()
            .map(|node| match &node.kind {
                NodeKind::Source => 0,
                NodeKind::Pass { dependencies, .. } => dependencies.len(),
            })
            .sum()
    }

    /// Validate and compile the subgraph reachable from `outputs`.
    pub fn compile(&self, outputs: &[NodeId]) -> Result<CompiledGraph, GraphError> {
        if outputs.is_empty() {
            return Err(GraphError::NoOutputs);
        }
        for output in outputs {
            if output.0 >= self.nodes.len() {
                return Err(GraphError::UnknownNode(*output));
            }
        }
        for (index, node) in self.nodes.iter().enumerate() {
            if matches!(node.kind, NodeKind::Pass { ref dependencies, .. } if dependencies.is_empty())
            {
                return Err(GraphError::PassHasNoInputs(NodeId(index)));
            }
        }

        let mut indegree = vec![0usize; self.nodes.len()];
        let mut consumers = vec![Vec::new(); self.nodes.len()];
        for (index, node) in self.nodes.iter().enumerate() {
            if let NodeKind::Pass { dependencies, .. } = &node.kind {
                indegree[index] = dependencies.len();
                for dependency in dependencies {
                    consumers[dependency.source.0].push(NodeId(index));
                }
            }
        }
        let mut ready: VecDeque<_> = indegree
            .iter()
            .enumerate()
            .filter_map(|(index, count)| (*count == 0).then_some(NodeId(index)))
            .collect();
        let mut all_topological = Vec::with_capacity(self.nodes.len());
        while let Some(node) = ready.pop_front() {
            all_topological.push(node);
            for consumer in &consumers[node.0] {
                indegree[consumer.0] -= 1;
                if indegree[consumer.0] == 0 {
                    ready.push_back(*consumer);
                }
            }
        }
        if all_topological.len() != self.nodes.len() {
            let nodes = indegree
                .iter()
                .enumerate()
                .filter_map(|(index, count)| (*count > 0).then_some(NodeId(index)))
                .collect();
            return Err(GraphError::Cycle { nodes });
        }

        let mut reachable = HashSet::new();
        let mut pending = outputs.to_vec();
        while let Some(node) = pending.pop() {
            if !reachable.insert(node) {
                continue;
            }
            if let NodeKind::Pass { dependencies, .. } = &self.nodes[node.0].kind {
                pending.extend(dependencies.iter().map(|edge| edge.source));
            }
        }
        let topological: Vec<_> = all_topological
            .into_iter()
            .filter(|node| reachable.contains(node))
            .collect();
        let targets = assign_targets(&self.nodes, &topological, outputs);

        Ok(CompiledGraph {
            nodes: self.nodes.clone(),
            outputs: outputs.to_vec(),
            topological,
            targets,
            limits: self.limits,
        })
    }
}

fn assign_targets(
    nodes: &[Node],
    topological: &[NodeId],
    outputs: &[NodeId],
) -> Vec<Option<TargetId>> {
    #[derive(Clone, Copy)]
    struct Slot {
        image: ImageDesc,
        available_after: usize,
        persistent: bool,
    }

    let positions: HashMap<_, _> = topological
        .iter()
        .enumerate()
        .map(|(position, node)| (*node, position))
        .collect();
    let mut last_use = vec![0usize; nodes.len()];
    for node in topological {
        let position = positions[node];
        last_use[node.0] = position;
        if let NodeKind::Pass { dependencies, .. } = &nodes[node.0].kind {
            for dependency in dependencies {
                last_use[dependency.source.0] = last_use[dependency.source.0].max(position);
            }
        }
    }
    for output in outputs {
        if let Some(position) = positions.get(output) {
            last_use[output.0] = last_use[output.0].max(position.saturating_add(1));
        }
    }

    let mut slots: Vec<Slot> = Vec::new();
    let mut assignments = vec![None; nodes.len()];
    for node in topological {
        let position = positions[node];
        let NodeKind::Pass { storage, .. } = nodes[node.0].kind else {
            continue;
        };
        let persistent = storage == Storage::Persistent;
        let reusable = (!persistent).then(|| {
            slots.iter().position(|slot| {
                !slot.persistent
                    && slot.image == nodes[node.0].image
                    && slot.available_after < position
            })
        });
        let target_index = reusable.flatten().unwrap_or_else(|| {
            slots.push(Slot {
                image: nodes[node.0].image,
                available_after: last_use[node.0],
                persistent,
            });
            slots.len() - 1
        });
        slots[target_index].available_after = last_use[node.0];
        assignments[node.0] = Some(TargetId(target_index));
    }
    assignments
}

/// Dynamic inputs used to plan one frame.
#[derive(Clone, Debug, Default)]
pub struct FrameRequest {
    outputs: HashMap<NodeId, RegionSet>,
    source_damage: HashMap<NodeId, RegionSet>,
    valid_persistent: HashSet<NodeId>,
    invalidated: HashSet<NodeId>,
}

impl FrameRequest {
    pub fn new() -> Self {
        Self::default()
    }

    /// Restrict an output to the requested regions. An output omitted from
    /// the request defaults to its complete declared domain.
    pub fn request_output(&mut self, output: NodeId, regions: RegionSet) -> &mut Self {
        self.outputs.insert(output, regions);
        self
    }

    pub fn damage_source(&mut self, source: NodeId, regions: RegionSet) -> &mut Self {
        self.source_damage.insert(source, regions);
        self
    }

    /// Record that a persistent output already contains valid pixels for this
    /// frame slot. Validity and damage are separate: a valid target may still
    /// need a partial rewrite.
    pub fn mark_persistent_valid(&mut self, node: NodeId) -> &mut Self {
        self.valid_persistent.insert(node);
        self
    }

    /// Invalidate an operator because its material, shader, geometry, or
    /// other non-image input changed.
    pub fn invalidate(&mut self, node: NodeId) -> &mut Self {
        self.invalidated.insert(node);
        self
    }
}

/// Immutable, validated graph and resource schedule.
#[derive(Clone, Debug)]
pub struct CompiledGraph {
    nodes: Vec<Node>,
    outputs: Vec<NodeId>,
    topological: Vec<NodeId>,
    targets: Vec<Option<TargetId>>,
    limits: GraphLimits,
}

impl CompiledGraph {
    pub fn topological_nodes(&self) -> &[NodeId] {
        &self.topological
    }

    pub fn label(&self, node: NodeId) -> Option<&str> {
        self.nodes.get(node.0).map(|node| node.label.as_str())
    }

    pub fn image_desc(&self, node: NodeId) -> Option<ImageDesc> {
        self.nodes.get(node.0).map(|node| node.image)
    }

    /// Storage policy of a pass. External sources return `None`.
    pub fn storage(&self, node: NodeId) -> Option<Storage> {
        self.nodes.get(node.0).and_then(|node| match node.kind {
            NodeKind::Source => None,
            NodeKind::Pass { storage, .. } => Some(storage),
        })
    }

    /// Explicit input nodes and coordinate maps of a pass.
    /// External sources and unknown nodes return `None`.
    pub fn dependencies(
        &self,
        node: NodeId,
    ) -> Option<impl ExactSizeIterator<Item = (NodeId, RegionMap)> + '_> {
        self.nodes.get(node.0).and_then(|node| match &node.kind {
            NodeKind::Source => None,
            NodeKind::Pass { dependencies, .. } => Some(
                dependencies
                    .iter()
                    .map(|dependency| (dependency.source, dependency.map)),
            ),
        })
    }

    pub fn target(&self, node: NodeId) -> Option<TargetId> {
        self.targets.get(node.0).copied().flatten()
    }

    pub fn target_count(&self) -> usize {
        self.targets
            .iter()
            .filter_map(|target| target.map(TargetId::index))
            .max()
            .map_or(0, |maximum| maximum + 1)
    }

    /// Compatibility descriptor for one physical target assignment.
    pub fn target_desc(&self, target: TargetId) -> Option<ImageDesc> {
        self.topological.iter().find_map(|node| {
            (self.targets[node.0] == Some(target)).then_some(self.nodes[node.0].image)
        })
    }

    pub fn plan_frame(&self, request: &FrameRequest) -> Result<FramePlan, GraphError> {
        self.validate_request(request)?;
        let count = self.nodes.len();
        let limit = self.limits.max_regions_per_node;
        let mut required = vec![RegionSet::new(); count];
        for output in &self.outputs {
            let regions = request
                .outputs
                .get(output)
                .cloned()
                .unwrap_or_else(|| RegionSet::from_rect(self.nodes[output.0].domain))
                .intersect(self.nodes[output.0].domain, limit);
            required[output.0].union_with(&regions, limit);
        }
        for node in self.topological.iter().rev() {
            let NodeKind::Pass { dependencies, .. } = &self.nodes[node.0].kind else {
                continue;
            };
            for dependency in dependencies {
                let input = dependency.map.required_input(
                    &required[node.0],
                    self.nodes[dependency.source.0].image,
                    self.nodes[node.0].image,
                    limit,
                );
                required[dependency.source.0].union_with(&input, limit);
            }
        }

        let mut damage = vec![RegionSet::new(); count];
        for node in &self.topological {
            match &self.nodes[node.0].kind {
                NodeKind::Source => {
                    if let Some(source_damage) = request.source_damage.get(node) {
                        damage[node.0] = source_damage
                            .intersect(self.nodes[node.0].domain, limit)
                            .intersect_required(&required[node.0], limit);
                    }
                }
                NodeKind::Pass {
                    storage,
                    dependencies,
                } => {
                    let valid =
                        *storage == Storage::Transient || request.valid_persistent.contains(node);
                    if !valid || request.invalidated.contains(node) {
                        damage[node.0] = required[node.0].clone();
                        continue;
                    }
                    for dependency in dependencies {
                        let propagated = dependency.map.damaged_output(
                            &damage[dependency.source.0],
                            self.nodes[dependency.source.0].image,
                            self.nodes[node.0].image,
                            self.nodes[node.0].domain,
                            limit,
                        );
                        damage[node.0].union_with(&propagated, limit);
                    }
                    damage[node.0] = damage[node.0].intersect_required(&required[node.0], limit);
                }
            }
        }

        // Derive actual pass work after damage is known. A valid cached sink
        // must not wake transient ancestors merely because the sink is still
        // logically requested. Persistent passes rewrite their damage;
        // transient graph outputs render their requested region. Work then
        // propagates backwards only through transient dependencies because a
        // valid persistent dependency can be sampled from its cache.
        let mut work = vec![RegionSet::new(); count];
        for node in &self.topological {
            let NodeKind::Pass { storage, .. } = self.nodes[node.0].kind else {
                continue;
            };
            match storage {
                Storage::Persistent => work[node.0] = damage[node.0].clone(),
                Storage::Transient if self.outputs.contains(node) => {
                    work[node.0] = required[node.0].clone();
                }
                Storage::Transient => {}
            }
        }
        for node in self.topological.iter().rev() {
            if work[node.0].is_empty() {
                continue;
            }
            let NodeKind::Pass { dependencies, .. } = &self.nodes[node.0].kind else {
                continue;
            };
            for dependency in dependencies {
                if !matches!(
                    self.nodes[dependency.source.0].kind,
                    NodeKind::Pass {
                        storage: Storage::Transient,
                        ..
                    }
                ) {
                    continue;
                }
                let input = dependency.map.required_input(
                    &work[node.0],
                    self.nodes[dependency.source.0].image,
                    self.nodes[node.0].image,
                    limit,
                );
                work[dependency.source.0].union_with(&input, limit);
            }
        }

        let steps = self
            .topological
            .iter()
            .filter_map(|node| {
                let NodeKind::Pass { .. } = self.nodes[node.0].kind else {
                    return None;
                };
                Some(ExecutionStep {
                    node: *node,
                    target: self.targets[node.0]
                        .expect("compiled pass must have a target assignment"),
                    required: required[node.0].clone(),
                    damage: damage[node.0].clone(),
                    work: work[node.0].clone(),
                    execute: !work[node.0].is_empty(),
                })
            })
            .collect();
        Ok(FramePlan {
            required,
            damage,
            steps,
        })
    }

    fn validate_request(&self, request: &FrameRequest) -> Result<(), GraphError> {
        for node in request
            .outputs
            .keys()
            .chain(request.source_damage.keys())
            .chain(request.valid_persistent.iter())
            .chain(request.invalidated.iter())
        {
            if node.0 >= self.nodes.len() {
                return Err(GraphError::UnknownNode(*node));
            }
        }
        for output in request.outputs.keys() {
            if !self.outputs.contains(output) {
                return Err(GraphError::NotAnOutput(*output));
            }
        }
        for source in request.source_damage.keys() {
            if !matches!(self.nodes[source.0].kind, NodeKind::Source) {
                return Err(GraphError::NotASource(*source));
            }
        }
        for node in &request.valid_persistent {
            if !matches!(
                self.nodes[node.0].kind,
                NodeKind::Pass {
                    storage: Storage::Persistent,
                    ..
                }
            ) {
                return Err(GraphError::NotPersistent(*node));
            }
        }
        Ok(())
    }
}

impl RegionSet {
    fn intersect_required(&self, required: &Self, limit: usize) -> Self {
        let mut result = Self::new();
        for left in &self.rects {
            for right in &required.rects {
                if let Some(rect) = left.intersect(*right) {
                    result.add(rect, limit);
                }
            }
        }
        result
    }
}

/// One executable pass in topological order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionStep {
    pub node: NodeId,
    pub target: TargetId,
    pub required: RegionSet,
    pub damage: RegionSet,
    /// Exact output region this pass must produce during the frame. For a
    /// persistent pass this is its damaged requested region. For a transient
    /// pass it is the region required by executing consumers.
    pub work: RegionSet,
    pub execute: bool,
}

/// Per-frame required regions, damage, and pass execution decisions.
#[derive(Clone, Debug)]
pub struct FramePlan {
    required: Vec<RegionSet>,
    damage: Vec<RegionSet>,
    steps: Vec<ExecutionStep>,
}

impl FramePlan {
    pub fn required(&self, node: NodeId) -> Option<&RegionSet> {
        self.required.get(node.0)
    }

    pub fn damage(&self, node: NodeId) -> Option<&RegionSet> {
        self.damage.get(node.0)
    }

    pub fn steps(&self) -> &[ExecutionStep] {
        &self.steps
    }

    pub fn step(&self, node: NodeId) -> Option<&ExecutionStep> {
        self.steps.iter().find(|step| step.node == node)
    }
}

/// Structural or per-frame validation failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GraphError {
    NodeLimit { limit: usize },
    EdgeLimit { limit: usize },
    InvalidImage(ImageDesc),
    EmptyDomain,
    UnknownNode(NodeId),
    SourceHasDependencies(NodeId),
    DuplicateDependency { pass: NodeId, source: NodeId },
    PassHasNoInputs(NodeId),
    NoOutputs,
    Cycle { nodes: Vec<NodeId> },
    NotAnOutput(NodeId),
    NotASource(NodeId),
    NotPersistent(NodeId),
}

impl fmt::Display for GraphError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NodeLimit { limit } => write!(formatter, "graph exceeds {limit} nodes"),
            Self::EdgeLimit { limit } => write!(formatter, "graph exceeds {limit} edges"),
            Self::InvalidImage(image) => write!(formatter, "invalid image descriptor {image:?}"),
            Self::EmptyDomain => formatter.write_str("pass domain is empty or outside its image"),
            Self::UnknownNode(node) => write!(formatter, "unknown node {node:?}"),
            Self::SourceHasDependencies(node) => {
                write!(formatter, "source node {node:?} cannot have dependencies")
            }
            Self::DuplicateDependency { pass, source } => {
                write!(formatter, "pass {pass:?} already depends on {source:?}")
            }
            Self::PassHasNoInputs(node) => write!(formatter, "pass {node:?} has no inputs"),
            Self::NoOutputs => formatter.write_str("graph has no outputs"),
            Self::Cycle { nodes } => write!(formatter, "graph contains a cycle through {nodes:?}"),
            Self::NotAnOutput(node) => write!(formatter, "node {node:?} is not a graph output"),
            Self::NotASource(node) => write!(formatter, "node {node:?} is not a source"),
            Self::NotPersistent(node) => {
                write!(formatter, "node {node:?} is not a persistent pass")
            }
        }
    }
}

impl Error for GraphError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(width: u32, height: u32) -> ImageDesc {
        ImageDesc::new(width, height, 1)
    }

    fn pass(
        graph: &mut CompositionGraph,
        label: &str,
        source: NodeId,
        storage: Storage,
        map: RegionMap,
    ) -> NodeId {
        let image = image(100, 100);
        let node = graph
            .add_pass(label, image, image.bounds(), storage)
            .unwrap();
        graph.add_dependency(node, source, map).unwrap();
        node
    }

    #[test]
    fn compiles_dependencies_before_consumers_and_drops_unreachable_nodes() {
        let mut graph = CompositionGraph::default();
        let source = graph.add_source("desktop", image(100, 100)).unwrap();
        let frost = pass(
            &mut graph,
            "frost",
            source,
            Storage::Transient,
            RegionMap::identity(),
        );
        let glass = pass(
            &mut graph,
            "glass",
            frost,
            Storage::Persistent,
            RegionMap::identity(),
        );
        let _unused = pass(
            &mut graph,
            "unused",
            source,
            Storage::Transient,
            RegionMap::identity(),
        );

        let compiled = graph.compile(&[glass]).unwrap();
        assert_eq!(compiled.topological_nodes(), &[source, frost, glass]);
    }

    #[test]
    fn rejects_cycles_even_when_the_cycle_is_not_an_output() {
        let mut graph = CompositionGraph::default();
        let source = graph.add_source("desktop", image(100, 100)).unwrap();
        let output = pass(
            &mut graph,
            "output",
            source,
            Storage::Persistent,
            RegionMap::identity(),
        );
        let a = graph
            .add_pass(
                "cycle-a",
                image(100, 100),
                image(100, 100).bounds(),
                Storage::Transient,
            )
            .unwrap();
        let b = graph
            .add_pass(
                "cycle-b",
                image(100, 100),
                image(100, 100).bounds(),
                Storage::Transient,
            )
            .unwrap();
        graph.add_dependency(a, b, RegionMap::identity()).unwrap();
        graph.add_dependency(b, a, RegionMap::identity()).unwrap();

        assert!(matches!(
            graph.compile(&[output]),
            Err(GraphError::Cycle { .. })
        ));
    }

    #[test]
    fn reverse_roi_accumulates_each_layers_sampling_radius() {
        let mut graph = CompositionGraph::default();
        let source = graph.add_source("desktop", image(100, 100)).unwrap();
        let blur = pass(
            &mut graph,
            "blur",
            source,
            Storage::Transient,
            RegionMap::local(4, 4),
        );
        let glass = pass(
            &mut graph,
            "glass",
            blur,
            Storage::Persistent,
            RegionMap::local(2, 2),
        );
        let compiled = graph.compile(&[glass]).unwrap();
        let mut request = FrameRequest::new();
        request.request_output(glass, RegionSet::from_rect(Rect::new(20, 20, 10, 10)));

        let plan = compiled.plan_frame(&request).unwrap();
        assert_eq!(
            plan.required(glass).unwrap().as_slice(),
            &[Rect::new(20, 20, 10, 10)]
        );
        assert_eq!(
            plan.required(blur).unwrap().as_slice(),
            &[Rect::new(18, 18, 14, 14)]
        );
        assert_eq!(
            plan.required(source).unwrap().as_slice(),
            &[Rect::new(14, 14, 22, 22)]
        );
    }

    #[test]
    fn forward_damage_accumulates_response_radius_and_clips_to_roi() {
        let mut graph = CompositionGraph::default();
        let source = graph.add_source("desktop", image(100, 100)).unwrap();
        let blur = pass(
            &mut graph,
            "blur",
            source,
            Storage::Persistent,
            RegionMap::local(4, 4),
        );
        let glass = pass(
            &mut graph,
            "glass",
            blur,
            Storage::Persistent,
            RegionMap::local(2, 2),
        );
        let compiled = graph.compile(&[glass]).unwrap();
        let mut request = FrameRequest::new();
        request
            .damage_source(source, RegionSet::from_rect(Rect::new(20, 20, 2, 2)))
            .mark_persistent_valid(blur)
            .mark_persistent_valid(glass);

        let plan = compiled.plan_frame(&request).unwrap();
        assert_eq!(
            plan.damage(blur).unwrap().as_slice(),
            &[Rect::new(16, 16, 10, 10)]
        );
        assert_eq!(
            plan.damage(glass).unwrap().as_slice(),
            &[Rect::new(14, 14, 14, 14)]
        );
    }

    #[test]
    fn invalidating_one_material_rewrites_only_its_requested_output() {
        let mut graph = CompositionGraph::default();
        let source = graph.add_source("desktop", image(100, 100)).unwrap();
        let glass = pass(
            &mut graph,
            "glass",
            source,
            Storage::Persistent,
            RegionMap::identity(),
        );
        let compiled = graph.compile(&[glass]).unwrap();
        let mut request = FrameRequest::new();
        request
            .request_output(glass, RegionSet::from_rect(Rect::new(40, 30, 20, 10)))
            .mark_persistent_valid(glass)
            .invalidate(glass);

        let plan = compiled.plan_frame(&request).unwrap();
        assert_eq!(
            plan.damage(glass).unwrap().as_slice(),
            &[Rect::new(40, 30, 20, 10)]
        );
        assert!(plan.step(glass).unwrap().execute);
    }

    #[test]
    fn valid_unchanged_persistent_pass_is_skipped() {
        let mut graph = CompositionGraph::default();
        let source = graph.add_source("desktop", image(100, 100)).unwrap();
        let glass = pass(
            &mut graph,
            "glass",
            source,
            Storage::Persistent,
            RegionMap::identity(),
        );
        let compiled = graph.compile(&[glass]).unwrap();
        let mut request = FrameRequest::new();
        request.mark_persistent_valid(glass);

        let plan = compiled.plan_frame(&request).unwrap();
        assert!(!plan.step(glass).unwrap().execute);
        assert!(plan.damage(glass).unwrap().is_empty());
    }

    #[test]
    fn cached_sink_does_not_wake_its_transient_ancestors() {
        let mut graph = CompositionGraph::default();
        let source = graph.add_source("desktop", image(100, 100)).unwrap();
        let blur = pass(
            &mut graph,
            "blur",
            source,
            Storage::Transient,
            RegionMap::local(4, 4),
        );
        let glass = pass(
            &mut graph,
            "glass",
            blur,
            Storage::Persistent,
            RegionMap::identity(),
        );
        let compiled = graph.compile(&[glass]).unwrap();
        let mut request = FrameRequest::new();
        request.mark_persistent_valid(glass);

        let cached = compiled.plan_frame(&request).unwrap();
        assert!(!cached.step(blur).unwrap().execute);
        assert!(!cached.step(glass).unwrap().execute);

        request.invalidate(glass);
        let invalidated = compiled.plan_frame(&request).unwrap();
        assert!(invalidated.step(blur).unwrap().execute);
        assert!(invalidated.step(glass).unwrap().execute);
        assert_eq!(
            invalidated.step(blur).unwrap().work.as_slice(),
            &[Rect::new(0, 0, 100, 100)]
        );
    }

    #[test]
    fn transient_targets_alias_only_after_the_last_read() {
        let mut graph = CompositionGraph::default();
        let source = graph.add_source("desktop", image(100, 100)).unwrap();
        let a = pass(
            &mut graph,
            "a",
            source,
            Storage::Transient,
            RegionMap::identity(),
        );
        let b = pass(
            &mut graph,
            "b",
            a,
            Storage::Transient,
            RegionMap::identity(),
        );
        let c = pass(
            &mut graph,
            "c",
            b,
            Storage::Transient,
            RegionMap::identity(),
        );
        let output = pass(
            &mut graph,
            "output",
            c,
            Storage::Persistent,
            RegionMap::identity(),
        );
        let compiled = graph.compile(&[output]).unwrap();

        assert_ne!(compiled.target(a), compiled.target(b));
        assert_eq!(compiled.target(a), compiled.target(c));
        assert_ne!(compiled.target(output), compiled.target(c));
        assert_eq!(compiled.target_count(), 3);
        assert_eq!(
            compiled.target_desc(compiled.target(a).unwrap()),
            Some(image(100, 100))
        );
        assert_eq!(compiled.storage(source), None);
        assert_eq!(compiled.storage(output), Some(Storage::Persistent));
        assert_eq!(
            compiled.dependencies(output).unwrap().collect::<Vec<_>>(),
            vec![(c, RegionMap::identity())]
        );
    }

    #[test]
    fn target_aliasing_respects_image_compatibility_class() {
        let mut graph = CompositionGraph::default();
        let source = graph.add_source("desktop", image(100, 100)).unwrap();
        let a = pass(
            &mut graph,
            "a",
            source,
            Storage::Transient,
            RegionMap::identity(),
        );
        let b = pass(
            &mut graph,
            "b",
            a,
            Storage::Transient,
            RegionMap::identity(),
        );
        let different = ImageDesc::new(100, 100, 2);
        let c = graph
            .add_pass("c", different, different.bounds(), Storage::Transient)
            .unwrap();
        graph.add_dependency(c, b, RegionMap::identity()).unwrap();
        let compiled = graph.compile(&[c]).unwrap();

        assert_ne!(compiled.target(a), compiled.target(c));
    }

    #[test]
    fn excessive_region_fragmentation_collapses_conservatively() {
        let limits = GraphLimits {
            max_regions_per_node: 2,
            ..GraphLimits::default()
        };
        let mut graph = CompositionGraph::new(limits);
        let source = graph.add_source("desktop", image(100, 100)).unwrap();
        let output = pass(
            &mut graph,
            "output",
            source,
            Storage::Persistent,
            RegionMap::identity(),
        );
        let compiled = graph.compile(&[output]).unwrap();
        let mut request = FrameRequest::new();
        request.request_output(
            output,
            RegionSet::from_rects([
                Rect::new(0, 0, 1, 1),
                Rect::new(10, 10, 1, 1),
                Rect::new(20, 20, 1, 1),
            ]),
        );

        let plan = compiled.plan_frame(&request).unwrap();
        assert_eq!(
            plan.required(output).unwrap().as_slice(),
            &[Rect::new(0, 0, 21, 21)]
        );
    }

    #[test]
    fn scaling_uses_enclosing_integer_coordinates() {
        let mut graph = CompositionGraph::default();
        let source = graph.add_source("half", image(50, 50)).unwrap();
        let output_image = image(100, 100);
        let output = graph
            .add_pass(
                "full",
                output_image,
                output_image.bounds(),
                Storage::Persistent,
            )
            .unwrap();
        graph
            .add_dependency(output, source, RegionMap::identity())
            .unwrap();
        let compiled = graph.compile(&[output]).unwrap();
        let mut request = FrameRequest::new();
        request.request_output(output, RegionSet::from_rect(Rect::new(1, 1, 2, 2)));

        let plan = compiled.plan_frame(&request).unwrap();
        assert_eq!(
            plan.required(source).unwrap().as_slice(),
            &[Rect::new(0, 0, 2, 2)]
        );
    }
}
