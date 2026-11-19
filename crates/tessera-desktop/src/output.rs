//! Output and monitor geometry (ADR-0028).
//!
//! Pure, backend- and renderer-agnostic model of one output's physical
//! mode, scale, and transform, plus the derivation of the logical size the
//! chrome and clients see. This is the foundation the multi-output milestone
//! (M7) and the tiling work-area build on; the workspace model's
//! [`Output`](crate::workspace::Output) gains a geometry reference when M7
//! wires real hotplug.

use tessera_primitives::{Point, Rect, Size, Transform};

const MILLIMETERS_PER_INCH: f32 = 25.4;
/// Lowest logical (post-scale) pixel density an automatic recommendation may
/// produce before it steps up. These floors are the only knob in the ladder:
/// they bound "the UI got too small", while `MAX_AUTO_SCALE` bounds "the UI
/// got too large". Internal panels are viewed closer and tolerate a higher
/// floor; external displays are normally viewed from further away, so they
/// accept a lower one (which is what makes 4K external panels land on 200%,
/// the mainstream desktop default, instead of a sub-1.5 fractional step).
const INTERNAL_MIN_LOGICAL_PPI: f32 = 105.0;
const EXTERNAL_MIN_LOGICAL_PPI: f32 = 80.0;
const MAX_AUTO_SCALE: u32 = 4;

/// Placement and backing class of an output (ADR-0028, ADR-0186).
///
/// Internal panels are normally viewed closer than external displays, so they
/// retain a slightly higher effective PPI floor after scaling. Virtual
/// outputs are headless extend connectors in the human interaction domain
/// (`VIRTUAL-*`), rendered into per-stream offscreen targets rather than the
/// physical KMS union framebuffer.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputKind {
    Internal,
    #[default]
    External,
    Virtual,
}

impl OutputKind {
    /// Whether this output is a headless virtual connector (`VIRTUAL-*`).
    pub fn is_virtual(self) -> bool {
        matches!(self, Self::Virtual)
    }

    /// Whether this output is backed by physical display hardware.
    pub fn is_physical(self) -> bool {
        !self.is_virtual()
    }

    /// Serde skip helper so default external outputs preserve existing wire
    /// fixtures.
    pub fn is_default(&self) -> bool {
        matches!(self, Self::External)
    }
}

/// How one connector participates in the desktop layout (ADR-0243).
///
/// A role is a *logical* relationship, not a copy of the framebuffer: an
/// extended output owns a distinct workspace strip and a distinct rectangle in
/// the global layout; a mirrored output is a presentation alias of a source
/// output and shares that source's rectangle, workspace strip, and input
/// target. The source is always an extended (or primary) physical output;
/// mirror chains are invalid.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(tag = "mode", rename_all = "snake_case"))]
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum OutputRole {
    /// Owns a distinct region of the logical desktop.
    #[default]
    Extend,
    /// Presents another output's logical region; owns no workspace.
    Mirror { source: String },
}

impl OutputRole {
    /// Whether this output presents another output's region.
    pub fn is_mirror(&self) -> bool {
        matches!(self, Self::Mirror { .. })
    }

    /// The connector this output mirrors, when it is a mirror.
    pub fn mirror_source(&self) -> Option<&str> {
        match self {
            Self::Mirror { source } => Some(source),
            Self::Extend => None,
        }
    }

    /// A human-readable role label for diagnostics and CLI output.
    pub fn label(&self) -> String {
        match self {
            Self::Extend => "extend".to_owned(),
            Self::Mirror { source } => format!("mirror of {source}"),
        }
    }
}

/// Whether an output's effective role came from an explicit `[[output]]`
/// entry or from the session topology policy.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputRoleSource {
    /// Resolved from `[display] topology`; the connector has no explicit role.
    #[default]
    Auto,
    /// The connector has an explicit `role` policy.
    Explicit,
}

impl OutputRoleSource {
    /// Resolve the role source for one connector from its policy.
    pub fn from_policy(policy: Option<&OutputPolicy>) -> Self {
        if policy.is_some_and(|policy| policy.role.is_some()) {
            Self::Explicit
        } else {
            Self::Auto
        }
    }

    pub fn is_auto(self) -> bool {
        matches!(self, Self::Auto)
    }
}

/// Session policy for outputs that carry no explicit [`OutputRole`] policy.
///
/// `Auto` is the shipped default: a smaller external display attached to a
/// session that already has a primary panel is treated as a presentation
/// target and mirrors the primary; a display larger than the primary — or an
/// equal-sized external paired with another external — extends the desktop.
/// Explicit per-connector roles always win. A connector without one is
/// re-evaluated on every connection, so applying a role in Settings is what
/// pins the choice.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DisplayTopology {
    /// Choose mirror or extend from the connected outputs' native pixel areas.
    Auto,
    /// Every output extends the desktop.
    Extend,
    /// Every output except the primary mirrors the primary.
    #[default]
    Mirror,
}

/// One coherent presentation group: outputs that must submit the same scene
/// generation together even though each owns an independent target and page
/// flip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresentationGroup {
    pub outputs: Vec<String>,
    /// Slowest real refresh in the group; `0` means unknown.
    pub refresh_mhz: u32,
}

impl PresentationGroup {
    pub fn period_ns(&self) -> Option<u64> {
        (self.refresh_mhz > 0).then(|| 1_000_000_000_000 / u64::from(self.refresh_mhz))
    }
}

/// Group physical outputs for coherent submission.
///
/// Mirrors always belong to their source group. Two extended outputs join the
/// same group when they share a refresh rate and touch at a logical edge, so a
/// window spanning the seam never tears. Disjoint outputs and mixed-refresh
/// neighbors remain independent groups. The group is a scheduling policy only:
/// it never shares a target, ring, lease, or page-flip slot.
pub fn presentation_groups(outputs: &[OutputInfo]) -> Vec<PresentationGroup> {
    presentation_groups_for_windows(outputs, &[])
}

/// Group physical outputs, additionally merging any groups that share a
/// visible window. A window spanning a seam therefore forces those groups into
/// one atomic presentation generation until the window stops spanning.
pub fn presentation_groups_for_windows(
    outputs: &[OutputInfo],
    windows: &[crate::window::Window],
) -> Vec<PresentationGroup> {
    let mut groups = base_presentation_groups(outputs);
    if groups.len() < 2 || windows.is_empty() {
        return groups;
    }
    let output_to_group: std::collections::HashMap<String, usize> = groups
        .iter()
        .enumerate()
        .flat_map(|(index, group)| {
            group
                .outputs
                .iter()
                .cloned()
                .map(move |connector| (connector, index))
        })
        .collect();
    let mut parent: Vec<usize> = (0..groups.len()).collect();
    fn find(parent: &mut [usize], mut index: usize) -> usize {
        while parent[index] != index {
            parent[index] = parent[parent[index]];
            index = parent[index];
        }
        index
    }
    for window in windows.iter().filter(|window| !window.minimized) {
        let rect = Rect {
            origin: window.position,
            size: window.size,
        };
        let mut touched = Vec::new();
        for output in outputs.iter().filter(|output| output.kind.is_physical()) {
            if output.geometry.logical_rect().intersect(rect).is_some()
                && let Some(&group) = output_to_group.get(&output.connector)
                && !touched.contains(&group)
            {
                touched.push(group);
            }
        }
        if let Some(&first) = touched.first() {
            for &group in &touched[1..] {
                let a = find(&mut parent, first);
                let b = find(&mut parent, group);
                if a != b {
                    parent[b] = a;
                }
            }
        }
    }
    let mut merged: Vec<PresentationGroup> = Vec::new();
    let mut merged_index: std::collections::HashMap<usize, usize> =
        std::collections::HashMap::new();
    for (index, group) in groups.drain(..).enumerate() {
        let root = find(&mut parent, index);
        if let Some(&target) = merged_index.get(&root) {
            merged[target].outputs.extend(group.outputs);
            merged[target].refresh_mhz =
                min_positive(merged[target].refresh_mhz, group.refresh_mhz);
        } else {
            merged_index.insert(root, merged.len());
            merged.push(group);
        }
    }
    merged
}

fn min_positive(a: u32, b: u32) -> u32 {
    match (a > 0, b > 0) {
        (true, true) => a.min(b),
        (true, false) => a,
        (false, true) => b,
        (false, false) => 0,
    }
}

fn base_presentation_groups(outputs: &[OutputInfo]) -> Vec<PresentationGroup> {
    let physical: Vec<&OutputInfo> = outputs
        .iter()
        .filter(|output| output.kind.is_physical())
        .collect();
    if physical.is_empty() {
        return Vec::new();
    }
    let mut parent: Vec<usize> = (0..physical.len()).collect();
    fn root(parent: &mut [usize], mut index: usize) -> usize {
        while parent[index] != index {
            parent[index] = parent[parent[index]];
            index = parent[index];
        }
        index
    }
    let union = |parent: &mut Vec<usize>, a: usize, b: usize| {
        let ra = root(parent, a);
        let rb = root(parent, b);
        if ra != rb {
            parent[rb] = ra;
        }
    };
    for (index, output) in physical.iter().enumerate() {
        if let Some(source) = output.role.mirror_source()
            && let Some(source_index) = physical
                .iter()
                .position(|candidate| candidate.connector == source)
        {
            union(&mut parent, index, source_index);
        }
    }
    for a in 0..physical.len() {
        for b in (a + 1)..physical.len() {
            if physical[a].geometry.mode.refresh_mhz != physical[b].geometry.mode.refresh_mhz {
                continue;
            }
            if rects_touch(
                physical[a].geometry.logical_rect(),
                physical[b].geometry.logical_rect(),
            ) {
                union(&mut parent, a, b);
            }
        }
    }
    let mut roots: Vec<usize> = Vec::new();
    for index in 0..physical.len() {
        let root = root(&mut parent, index);
        if !roots.contains(&root) {
            roots.push(root);
        }
    }
    roots
        .into_iter()
        .map(|root| {
            let members: Vec<&OutputInfo> = (0..physical.len())
                .filter(|index| root_of(&mut parent, *index) == root)
                .map(|index| physical[index])
                .collect();
            let refresh_mhz = members
                .iter()
                .filter_map(|output| {
                    (output.geometry.mode.refresh_mhz > 0)
                        .then_some(output.geometry.mode.refresh_mhz)
                })
                .min()
                .unwrap_or(0);
            PresentationGroup {
                outputs: members
                    .into_iter()
                    .map(|output| output.connector.clone())
                    .collect(),
                refresh_mhz,
            }
        })
        .collect()
}

fn root_of(parent: &mut [usize], mut index: usize) -> usize {
    while parent[index] != index {
        parent[index] = parent[parent[index]];
        index = parent[index];
    }
    index
}

fn rects_touch(a: Rect, b: Rect) -> bool {
    let a_right = a.origin.x.saturating_add(a.size.w);
    let b_right = b.origin.x.saturating_add(b.size.w);
    let a_bottom = a.origin.y.saturating_add(a.size.h);
    let b_bottom = b.origin.y.saturating_add(b.size.h);
    let vertical_overlap = a.origin.y < b_bottom && b.origin.y < a_bottom;
    let horizontal_overlap = a.origin.x < b_right && b.origin.x < a_right;
    (vertical_overlap && (a_right == b.origin.x || b_right == a.origin.x))
        || (horizontal_overlap && (a_bottom == b.origin.y || b_bottom == a.origin.y))
}

/// Presentation-period of one atomic display domain, in nanoseconds.
///
/// Tessera flips every active CRTC in a single atomic batch
/// ([`crate::output`] union layout; per-output presentation domains are
/// ADR-0185, a design that has not landed). The batch retires only when the
/// *slowest* output's page flip lands, so the slowest refresh rate bounds
/// how often the desktop can present — even the fastest output waits on the
/// slowest one for the whole batch to retire.
///
/// Every consumer of refresh timing must use this single derivation so the
/// compositor's *own* cadence (`PresentationScheduler`) and what it
/// *advertises* to clients via `wp_presentation_feedback.refresh` can never
/// disagree. Publishing a faster period than the domain actually presents
/// (e.g. the single fastest panel instead of the slowest) makes a client pace
/// to a cadence the compositor cannot honour, over-commit, and drop frames by
/// construction (Optics ADR-0122 / tessera ADR-0196).
///
/// Returns `None` when no output reports a real (non-zero) refresh rate — the
/// honest "unknown" answer for a nested/headless backend that owns no vblank.
#[must_use]
pub fn domain_refresh_period_ns(refresh_rates_mhz: impl IntoIterator<Item = u32>) -> Option<u64> {
    // A zero rate means "unknown" and must not monopolize a real set.
    let slowest_mhz = refresh_rates_mhz
        .into_iter()
        .filter(|rate| *rate > 0)
        .min()?;
    // refresh_mhz is millihertz: period_ns = 1e9 ns/s * 1e3 ms/s / mhz = 1e12 / mhz.
    Some(1_000_000_000_000 / u64::from(slowest_mhz))
}

/// Presentation-period of the physical atomic display domain, in nanoseconds.
///
/// This is the single definition of *which* outputs form the presentation
/// domain, paired with [`domain_refresh_period_ns`] as the single definition
/// of the arithmetic. Both halves belong together because deriving them
/// separately is how they drift: the compositor's scheduler once paced on
/// every output while `wp_presentation` reported only physical ones, so a
/// virtual output slower than the panel (e.g. a 24 Hz capture target) dragged
/// the advertised cadence and the real one apart in opposite directions.
///
/// **Virtual outputs are excluded by construction.** They are headless stream
/// targets rendered into per-output offscreen images (ADR-0186), never part of
/// the DRM atomic batch, so they cannot bound when the desktop presents. Their
/// own cadence is governed by stream `max_fps` pacing (ADR-0130), which forces
/// a presentation independently. Counting them here would let a slow recording
/// target throttle the physical desktop.
#[must_use]
pub fn presentation_domain_period_ns<'a>(
    outputs: impl IntoIterator<Item = &'a OutputInfo>,
) -> Option<u64> {
    domain_refresh_period_ns(
        outputs
            .into_iter()
            .filter(|output| output.kind.is_physical())
            .map(|output| output.geometry.mode.refresh_mhz),
    )
}

/// Prefix required on every human-domain virtual connector name (ADR-0186).
pub const VIRTUAL_CONNECTOR_PREFIX: &str = "VIRTUAL-";
/// Maximum byte length of a virtual connector identifier.
pub const MAX_VIRTUAL_CONNECTOR_LEN: usize = 64;
/// Minimum physical dimension (width or height) of a virtual output.
pub const MIN_VIRTUAL_OUTPUT_DIM: i32 = 64;
/// Maximum physical dimension (width or height) of a virtual output.
pub const MAX_VIRTUAL_OUTPUT_DIM: i32 = 16_384;
/// Maximum physical pixel area of a virtual output (256 MiB at 4 Bpp RGBA).
pub const MAX_VIRTUAL_OUTPUT_PIXELS: u64 = 67_108_864;

/// Whether `name` is a well-formed virtual output connector identifier
/// (`VIRTUAL-<suffix>` with ASCII alphanumeric, `'-'`, or `'_'` characters).
pub fn valid_virtual_connector_name(name: &str) -> bool {
    let Some(suffix) = name.strip_prefix(VIRTUAL_CONNECTOR_PREFIX) else {
        return false;
    };
    !suffix.is_empty()
        && name.len() <= MAX_VIRTUAL_CONNECTOR_LEN
        && suffix
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

/// Validate physical mode and scale bounds for a virtual output (ADR-0186).
pub fn validate_virtual_output_mode_and_scale(
    mode: OutputMode,
    scale: Scale,
) -> Result<(), &'static str> {
    if !(MIN_VIRTUAL_OUTPUT_DIM..=MAX_VIRTUAL_OUTPUT_DIM).contains(&mode.width)
        || !(MIN_VIRTUAL_OUTPUT_DIM..=MAX_VIRTUAL_OUTPUT_DIM).contains(&mode.height)
    {
        return Err("virtual output dimensions must be between 64 and 16384 pixels");
    }
    if !(1_000..=240_000).contains(&mode.refresh_mhz) {
        return Err("virtual output refresh rate must be between 1 Hz and 240 Hz");
    }
    let pixels = u64::from(mode.width as u32).saturating_mul(u64::from(mode.height as u32));
    if pixels > MAX_VIRTUAL_OUTPUT_PIXELS {
        return Err("virtual output pixel count exceeds the 256 MiB frame ceiling");
    }
    if !scale.0.is_finite() || !(0.25..=8.0).contains(&scale.0) {
        return Err("virtual output scale must be between 0.25 and 8.0");
    }
    Ok(())
}

/// Specification for creating a dynamic virtual extend output in the human
/// interaction domain (ADR-0186).
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq)]
pub struct VirtualOutputSpec {
    /// Optional connector name (`VIRTUAL-*`). When `None`, the compositor
    /// allocates the lowest unused `VIRTUAL-1`, `VIRTUAL-2`, ….
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub connector: Option<String>,
    /// Physical resolution and refresh rate of the virtual display.
    pub mode: OutputMode,
    /// Logical-to-physical scale factor (defaults to `1.0`).
    #[cfg_attr(feature = "serde", serde(default))]
    pub scale: Scale,
    /// Optional explicit top-left in global logical desktop coordinates;
    /// when `None`, the compositor places the output to the right of existing
    /// outputs.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub logical_origin: Option<Point>,
    /// When `false` (the default), the virtual output is bound to the
    /// creating IPC connection and automatically destroyed on disconnect
    /// (`[INV-ARCH-75]`). Portal-created outputs are always session-scoped;
    /// only owner-admin CLI requests may set `persistent = true`.
    #[cfg_attr(feature = "serde", serde(default))]
    pub persistent: bool,
}

impl VirtualOutputSpec {
    /// Validate all pure model bounds before handing the request to the
    /// compositor main loop.
    pub fn validate(&self) -> Result<(), &'static str> {
        if let Some(connector) = self.connector.as_deref()
            && !valid_virtual_connector_name(connector)
        {
            return Err("virtual output connector must match VIRTUAL-<name>");
        }
        if let Some(origin) = self.logical_origin
            && (!(-32_768..=32_768).contains(&origin.x) || !(-32_768..=32_768).contains(&origin.y))
        {
            return Err("virtual output logical origin is out of range");
        }
        validate_virtual_output_mode_and_scale(self.mode, self.scale)
    }
}

/// A display mode: physical resolution and refresh rate.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OutputMode {
    /// Physical width in device pixels.
    pub width: i32,
    /// Physical height in device pixels.
    pub height: i32,
    /// Refresh rate in millihertz (e.g. 60000 for 60 Hz), matching the
    /// DRM/KMS and `wl_output.mode` convention.
    pub refresh_mhz: u32,
}

/// A configured display-mode request (ADR-0028): an exact resolution with an
/// optional whole-Hertz refresh, parsed from the `mode` string of a
/// `[[output]]` config entry (`"1920x1080"` or `"2560x1440@144"`). The
/// backend matches it against the modes a connector advertises with
/// [`ModeSpec::matches`].
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModeSpec {
    /// Requested physical width in device pixels.
    pub width: i32,
    /// Requested physical height in device pixels.
    pub height: i32,
    /// Requested refresh in whole Hertz. `None` leaves the rate open: the
    /// backend picks the highest-refresh mode of that size.
    pub refresh_hz: Option<u32>,
}

impl ModeSpec {
    /// Whether an advertised mode satisfies this request. Resolution must
    /// match exactly; a named refresh matches when the mode's millihertz
    /// rate rounds to it, and `None` matches any rate.
    pub fn matches(&self, mode: &OutputMode) -> bool {
        if mode.width != self.width || mode.height != self.height {
            return false;
        }
        match self.refresh_hz {
            Some(hz) => mode.refresh_mhz.saturating_add(500) / 1_000 == hz,
            None => true,
        }
    }
}

impl std::str::FromStr for ModeSpec {
    type Err = ();

    /// Parse `"WxH"` or `"WxH@Hz"`. Only positive decimal integers are
    /// accepted — no whitespace, signs, or trailing garbage.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        fn positive_i32(s: &str) -> Result<i32, ()> {
            if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
                return Err(());
            }
            let value = s.parse::<i32>().map_err(|_| ())?;
            if value > 0 { Ok(value) } else { Err(()) }
        }
        let (size, refresh) = match s.split_once('@') {
            Some((size, hz)) => (size, Some(hz)),
            None => (s, None),
        };
        let (w, h) = size.split_once('x').ok_or(())?;
        Ok(ModeSpec {
            width: positive_i32(w)?,
            height: positive_i32(h)?,
            refresh_hz: refresh.map(positive_i32).transpose()?.map(|hz| hz as u32),
        })
    }
}

/// Per-connector output configuration policy (ADR-0028): the resolved form
/// of one `[[output]]` config entry. `scale`, `position`, and `primary` are
/// applied by the server as outputs appear; `mode` is consumed by the
/// backend at modeset time. `transform` is parsed and validated but its
/// application is deferred until renderer output-transform support lands.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Default)]
pub struct OutputPolicy {
    /// Explicit role. `None` leaves the connector to the session topology
    /// policy ([`DisplayTopology`]).
    pub role: Option<OutputRole>,
    /// Scale override; `None` keeps the backend-reported scale.
    pub scale: Option<f64>,
    /// Requested display mode; `None` selects the connector's highest-pixel
    /// mode at its highest refresh rate.
    pub mode: Option<ModeSpec>,
    /// Top-left in the global logical layout; `None` keeps the
    /// backend-assigned position.
    pub position: Option<Point>,
    /// Output transform. Parsed but not yet applied (see the type docs).
    pub transform: Option<Transform>,
    /// Whether this output is the primary (focused) one.
    pub primary: bool,
    /// Allow HDR (BT.2020 PQ) output on this connector. The compositor's
    /// shared framebuffer enters HDR mode only when *every* active output
    /// both allows it here and advertises ST 2084 support through EDID;
    /// anything less stays SDR.
    pub hdr: bool,
    /// Allow a 10-bit deep-color framebuffer (RGB10A2-class scanout) for
    /// reduced banding in SDR. Also gated on every active output allowing
    /// it and every primary plane supporting the format.
    pub deep_color: bool,
    /// Target SDR white luminance in cd/m² (nits) in HDR mode (ITU-R BT.2408
    /// default: 203.0). Valid range is 80.0 through 500.0.
    pub sdr_white_nits: Option<f32>,
}

/// Per-connector color policy (the `hdr` / `deep_color` / `sdr_white_nits` fields of a
/// `[[output]]` config entry), consumed by the backend at modeset time.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ColorPolicy {
    /// Allow HDR (BT.2020 PQ) on this connector.
    pub hdr: bool,
    /// Allow a 10-bit deep-color SDR framebuffer on this connector.
    pub deep_color: bool,
    /// Target SDR white luminance in cd/m² (nits) for HDR mode.
    pub sdr_white_nits: Option<f32>,
}

/// The session-wide color pipeline actually in effect (the compositor
/// renders one shared framebuffer, so the pixel encoding is uniform).
/// Reported by the backend; the compositor server derives the per-output
/// image description for `wp_color_management_v1` from it.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColorPipeline {
    /// 8-bit sRGB (the default).
    #[default]
    Sdr,
    /// 10-bit sRGB in an RGB10A2-class container (deep color).
    SdrDeepColor,
    /// BT.2020 PQ (HDR10-class) in an RGB10A2-class container.
    Hdr,
}

impl ColorPipeline {
    /// The output image description this pipeline presents to clients:
    /// the space the framebuffer is written in. The luminances anchor the
    /// description to absolute levels so clients can tell SDR from HDR
    /// (and scale headroom correctly) instead of guessing.
    pub fn output_color(self) -> tessera_primitives::color::ParametricColor {
        use tessera_primitives::color::{
            ContentPrimaries, ContentTransfer, Luminances, NamedPrimaries, NamedTransfer,
            ParametricColor,
        };
        match self {
            ColorPipeline::Sdr | ColorPipeline::SdrDeepColor => ParametricColor {
                luminances: Some(Luminances::SDR),
                ..tessera_primitives::color::ContentColor::SRGB
            },
            ColorPipeline::Hdr => ParametricColor {
                primaries: ContentPrimaries::Named(NamedPrimaries::Bt2020),
                transfer: ContentTransfer::Named(NamedTransfer::Pq),
                luminances: Some(Luminances::HDR),
                max_cll: None,
                max_fall: None,
            },
        }
    }
}

/// A scale factor. Carries a fractional value so an explicitly configured
/// non-integer density (1.5, 1.25) is representable, beyond the integer-only
/// `wl_surface.set_buffer_scale`. Maps to `wp_fractional_scale_v1`.
///
/// [`automatic_scale`] never produces a fractional value: a fractional scale
/// forces every client that does not speak `wp_fractional_scale_v1` to render
/// at `ceil(scale)` × its logical extent and disqualifies its buffer from
/// direct scanout (see that function's contract).
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scale(pub f32);

impl Scale {
    /// No scaling — logical and physical pixels coincide.
    pub const IDENTITY: Scale = Scale(1.0);

    /// The scale as an `f32`.
    pub fn as_f32(self) -> f32 {
        self.0
    }
}

impl Default for Scale {
    fn default() -> Scale {
        Scale::IDENTITY
    }
}

/// Calculate the diagonal pixel density from a mode and the display's
/// physical dimensions. Dimensions with an implausible size, density, or
/// pixel-to-millimeter aspect ratio are rejected because EDID data is not
/// universally reliable.
pub fn physical_ppi(mode: OutputMode, physical_size_mm: (u32, u32)) -> Option<f32> {
    let (pixel_width, pixel_height) = (mode.width as f32, mode.height as f32);
    let (mm_width, mm_height) = (physical_size_mm.0 as f32, physical_size_mm.1 as f32);
    if pixel_width <= 0.0
        || pixel_height <= 0.0
        || !(40.0..=3_000.0).contains(&mm_width)
        || !(40.0..=3_000.0).contains(&mm_height)
    {
        return None;
    }

    let ppi_x = pixel_width * MILLIMETERS_PER_INCH / mm_width;
    let ppi_y = pixel_height * MILLIMETERS_PER_INCH / mm_height;
    let axis_ratio = ppi_x.max(ppi_y) / ppi_x.min(ppi_y);
    if !axis_ratio.is_finite() || axis_ratio > 1.1 {
        return None;
    }

    let diagonal_pixels = pixel_width.hypot(pixel_height);
    let diagonal_inches = mm_width.hypot(mm_height) / MILLIMETERS_PER_INCH;
    let ppi = diagonal_pixels / diagonal_inches;
    (ppi.is_finite() && (40.0..=1_000.0).contains(&ppi)).then_some(ppi)
}

/// Recommend an automatic output scale from physical pixel density.
///
/// Recommendations are **integral only**. An output scale is fundamentally an
/// integer: `wl_output.scale` is an `i32`, and a client that does not speak
/// `wp_fractional_scale_v1` renders every surface at `ceil(scale)` × its
/// logical extent. A fractional recommendation would therefore make such a
/// client allocate a buffer larger than the output mode (1.78× the pixels at
/// 1.5, 2.56× at 1.25) — a pure fill-rate tax — and would leave that buffer
/// geometrically unequal to the mode extent, which permanently disqualifies
/// it from KMS direct scanout.
///
/// The ladder picks the largest integral step whose logical density stays at
/// or above the display class's floor, never shrinking below 100% and never
/// exceeding `MAX_AUTO_SCALE`. Fractional scales remain representable and
/// configurable per output (see [`OutputPolicy::scale`]); they are simply
/// never *recommended*.
pub fn automatic_scale(
    mode: OutputMode,
    physical_size_mm: (u32, u32),
    kind: OutputKind,
) -> Option<Scale> {
    if kind.is_virtual() {
        return Some(Scale::IDENTITY);
    }
    let ppi = physical_ppi(mode, physical_size_mm)?;
    let floor = match kind {
        OutputKind::Internal => INTERNAL_MIN_LOGICAL_PPI,
        OutputKind::External | OutputKind::Virtual => EXTERNAL_MIN_LOGICAL_PPI,
    };
    let mut scale = 1_u32;
    while scale < MAX_AUTO_SCALE && ppi / (scale + 1) as f32 >= floor {
        scale += 1;
    }
    Some(Scale(scale as f32))
}

/// One output's identity + geometry — the wire shape the IPC exposes for an
/// output. The connector is the stable identity; the geometry is its current
/// mode, scale, transform, and position.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq)]
pub struct OutputInfo {
    pub connector: String,
    pub geometry: OutputGeometry,
    /// Placement and backing class (`Internal`, `External`, or `Virtual`,
    /// ADR-0186). Defaults to `External` and skips serialization when default
    /// so existing wire fixtures stay unchanged.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "OutputKind::is_default")
    )]
    pub kind: OutputKind,
    /// Modes the connector advertises (deduplicated, highest resolution
    /// first), so `tessera display` and agents can see what `mode` requests
    /// are valid. Empty where the backend cannot enumerate (nested).
    /// `serde(default)` keeps pre-field IPC peers compatible.
    #[cfg_attr(feature = "serde", serde(default))]
    pub available_modes: Vec<OutputMode>,
    /// Color capabilities parsed from the display's EDID (HDR transfer
    /// support, BT.2020 colorimetry). All-false where the backend cannot
    /// read EDID (nested).
    #[cfg_attr(feature = "serde", serde(default))]
    pub color_caps: tessera_primitives::edid::EdidColorCapabilities,
    /// How this output participates in the logical desktop (ADR-0243).
    #[cfg_attr(feature = "serde", serde(default))]
    pub role: OutputRole,
    /// Whether `role` is explicit configuration or the auto policy's effective
    /// choice. Settings uses this to show and restore "Automatic".
    #[cfg_attr(feature = "serde", serde(default))]
    pub role_source: OutputRoleSource,
}

impl OutputInfo {
    /// Fraction of the panel that presents desktop content, in normalized
    /// output coordinates `(x, y, width, height)`.
    ///
    /// An extended output fills its panel. A mirror preserves the source
    /// aspect ratio, so a target whose aspect differs leaves black bars on
    /// one axis. Input devices mapped to this output must be inset by the
    /// same fraction; otherwise a touch outside the visible content would
    /// resolve to a desktop edge.
    pub fn content_fraction(&self) -> (f32, f32, f32, f32) {
        if !self.role.is_mirror() {
            return (0.0, 0.0, 1.0, 1.0);
        }
        let mode_w = self.geometry.mode.width.max(1) as f32;
        let mode_h = self.geometry.mode.height.max(1) as f32;
        let source = self.geometry.logical_size();
        let source_w = source.w.max(1) as f32;
        let source_h = source.h.max(1) as f32;
        let mode_aspect = mode_w / mode_h;
        let source_aspect = source_w / source_h;
        if !mode_aspect.is_finite() || !source_aspect.is_finite() {
            return (0.0, 0.0, 1.0, 1.0);
        }
        if mode_aspect >= source_aspect {
            let width = (source_aspect / mode_aspect).clamp(0.0, 1.0);
            ((1.0 - width) * 0.5, 0.0, width, 1.0)
        } else {
            let height = (mode_aspect / source_aspect).clamp(0.0, 1.0);
            (0.0, (1.0 - height) * 0.5, 1.0, height)
        }
    }
}

/// Per-output geometry (ADR-0028): the physical mode, scale, transform, and
/// the output's top-left in the global logical layout. From these the
/// [`logical_size`](Self::logical_size) — the size the chrome and clients
/// operate in — is derived.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OutputGeometry {
    pub mode: OutputMode,
    pub scale: Scale,
    pub transform: Transform,
    /// Top-left of this output in the global logical coordinate space. The
    /// primary output sits at (0, 0); others are placed relative to it.
    pub logical_origin: Point,
    /// Exact logical extent for a presentation alias (ADR-0243). A mirror
    /// shares its source output's logical rectangle even when its native mode
    /// and scale differ, so the derived `mode / scale` size is insufficient.
    /// `None` for ordinary extended outputs, which derive their size.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub logical_size_override: Option<Size>,
}

impl Default for OutputGeometry {
    fn default() -> OutputGeometry {
        OutputGeometry {
            mode: OutputMode {
                width: 0,
                height: 0,
                refresh_mhz: 0,
            },
            scale: Scale::IDENTITY,
            transform: Transform::Normal,
            logical_origin: Point::default(),
            logical_size_override: None,
        }
    }
}

impl OutputGeometry {
    /// The logical size the chrome and clients see: for a presentation alias,
    /// the source output's exact logical extent; otherwise the physical mode,
    /// axes swapped for a 90°/270° transform, divided by the scale. Rounded to
    /// the nearest logical pixel.
    pub fn logical_size(&self) -> Size {
        if let Some(size) = self.logical_size_override
            && size.w > 0
            && size.h > 0
        {
            return size;
        }
        let (w, h) = if self.transform.swap_axes() {
            (self.mode.height, self.mode.width)
        } else {
            (self.mode.width, self.mode.height)
        };
        let s = self.scale.0;
        if s <= 0.0 {
            // A non-positive scale is nonsensical; avoid divide-by-zero.
            return Size { w, h };
        }
        Size {
            w: ((w as f32) / s).round() as i32,
            h: ((h as f32) / s).round() as i32,
        }
    }

    /// The output's rect in the global logical layout.
    pub fn logical_rect(&self) -> Rect {
        Rect {
            origin: self.logical_origin,
            size: self.logical_size(),
        }
    }
}

/// Resolve the effective [`OutputRole`] of every output from explicit
/// per-connector policies and the session topology policy (ADR-0243).
///
/// The resolver is pure so the compositor server and the DRM backend can
/// agree on one topology without duplicating heuristics. Virtual outputs are
/// always extended; a mirror source is always a physical, non-mirrored output.
///
/// `Auto` mirrors the first additional external output when its native pixel
/// area is smaller than the primary panel's, or equal when the primary is an
/// internal panel. That is the common "laptop + smaller TV/projector" case.
/// Larger externals and equal-sized external pairs are desktop extensions.
/// Internal panels never auto-mirror.
pub fn resolve_output_roles(
    outputs: &[OutputInfo],
    policies: &std::collections::HashMap<String, OutputPolicy>,
    topology: DisplayTopology,
) -> Vec<OutputRole> {
    let explicit: Vec<Option<OutputRole>> = outputs
        .iter()
        .map(|output| {
            policies
                .get(&output.connector)
                .and_then(|policy| policy.role.clone())
        })
        .collect();
    let mut roles: Vec<OutputRole> = explicit
        .iter()
        .map(|role| role.clone().unwrap_or(OutputRole::Extend))
        .collect();
    let physical: Vec<usize> = outputs
        .iter()
        .enumerate()
        .filter(|(_, output)| output.kind.is_physical())
        .map(|(index, _)| index)
        .collect();
    if physical.is_empty() {
        return roles;
    }

    // An explicit primary policy wins; otherwise prefer the internal panel so
    // a docked laptop keeps its panel as the mirror source. A primary cannot
    // itself be a mirror: fall through to the next usable physical output.
    // Multiple `primary = true` entries are resolved by backend output order,
    // matching the server's reorder rule; iterating the policy map directly
    // would make the source depend on HashMap hash order.
    let mut preferred_source = outputs.iter().position(|output| {
        policies
            .get(&output.connector)
            .is_some_and(|policy| policy.primary)
    });
    if preferred_source.is_some_and(|index| roles.get(index).is_some_and(OutputRole::is_mirror)) {
        preferred_source = None;
    }
    let extended_physical = || {
        physical
            .iter()
            .copied()
            .filter(|index| !roles[*index].is_mirror())
    };
    let source = preferred_source
        .filter(|index| !roles[*index].is_mirror())
        .or_else(|| extended_physical().find(|index| outputs[*index].kind == OutputKind::Internal))
        .or_else(|| extended_physical().next())
        .unwrap_or(physical[0]);
    if roles.get(source).is_none_or(OutputRole::is_mirror) {
        return roles;
    }

    // Validate explicit mirror sources before auto-assigning anything: a
    // source must exist, be physical, not be the output itself, and must not
    // itself be a mirror. Invalid configuration degrades to Extend.
    for (index, role) in explicit.iter().enumerate() {
        let Some(OutputRole::Mirror { source }) = role else {
            continue;
        };
        let valid = outputs
            .iter()
            .position(|output| output.connector == *source)
            .is_some_and(|source_index| {
                source_index != index
                    && outputs[source_index].kind.is_physical()
                    && !roles[source_index].is_mirror()
                    && !matches!(explicit[source_index], Some(OutputRole::Mirror { .. }))
            });
        if !valid {
            roles[index] = OutputRole::Extend;
        }
    }
    // A source that an explicit mirror selected must stay extended even when
    // the session topology would otherwise mirror it.
    let pinned_sources: std::collections::HashSet<usize> = roles
        .iter()
        .filter_map(|role| match role {
            OutputRole::Mirror { source } => outputs
                .iter()
                .position(|output| output.connector == *source),
            OutputRole::Extend => None,
        })
        .collect();
    for &source_index in &pinned_sources {
        roles[source_index] = OutputRole::Extend;
    }

    let source_area = native_area(&outputs[source]);
    let source_internal = outputs[source].kind == OutputKind::Internal;
    let mut auto_mirrored = false;
    for index in physical {
        if index == source || explicit[index].is_some() || pinned_sources.contains(&index) {
            continue;
        }
        roles[index] = match topology {
            DisplayTopology::Extend => OutputRole::Extend,
            DisplayTopology::Mirror => OutputRole::Mirror {
                source: outputs[source].connector.clone(),
            },
            DisplayTopology::Auto => {
                let target_area = native_area(&outputs[index]);
                // Equal-area external pairs are the classic side-by-side
                // desktop; only an internal source treats an equal-size
                // external as a presentation target. A strictly smaller
                // external is a TV/projector in either case.
                let eligible = !auto_mirrored
                    && outputs[index].kind == OutputKind::External
                    && (target_area < source_area
                        || (source_internal && target_area == source_area));
                if eligible {
                    auto_mirrored = true;
                    OutputRole::Mirror {
                        source: outputs[source].connector.clone(),
                    }
                } else {
                    OutputRole::Extend
                }
            }
        };
    }
    roles
}

/// Native pixel area used by the automatic topology heuristic. Prefer the
/// connector's advertised mode list so a configured lower mode cannot change
/// whether the display is treated as a presentation target; fall back to the
/// selected mode when the backend reports no list (nested/headless).
fn native_area(output: &OutputInfo) -> u64 {
    let area = |mode: OutputMode| {
        u64::from(mode.width.max(0) as u32) * u64::from(mode.height.max(0) as u32)
    };
    output
        .available_modes
        .iter()
        .map(|mode| area(*mode))
        .max()
        .unwrap_or_else(|| area(output.geometry.mode))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode(w: i32, h: i32) -> OutputMode {
        OutputMode {
            width: w,
            height: h,
            refresh_mhz: 60000,
        }
    }

    #[test]
    fn domain_refresh_period_is_bounded_by_the_slowest_output() {
        // The atomic batch retires only when the slowest CRTC flips, so a
        // 165 Hz + 60 Hz pair presents at 60 Hz. 1e12 / 60_000 = 16_666_666 ns.
        assert_eq!(
            domain_refresh_period_ns([165_000, 60_000]),
            Some(16_666_666)
        );
        // A single output is its own slowest.
        assert_eq!(domain_refresh_period_ns([60_000]), Some(16_666_666));
        assert_eq!(domain_refresh_period_ns([144_000]), Some(6_944_444));
    }

    #[test]
    fn domain_refresh_period_ignores_unknown_rates() {
        // 0 is the "unknown" sentinel a nested/headless output reports; it must
        // not win the `min` and fabricate a period.
        assert_eq!(domain_refresh_period_ns([0, 60_000]), Some(16_666_666));
        // With no real rate at all the honest answer is "unknown".
        assert_eq!(domain_refresh_period_ns([0, 0]), None);
        assert_eq!(domain_refresh_period_ns(std::iter::empty()), None);
    }

    fn info(connector: &str, refresh_mhz: u32, kind: OutputKind) -> OutputInfo {
        OutputInfo {
            connector: connector.to_owned(),
            geometry: OutputGeometry {
                mode: OutputMode {
                    width: 1920,
                    height: 1080,
                    refresh_mhz,
                },
                logical_size_override: None,
                ..Default::default()
            },
            kind,
            available_modes: Vec::new(),
            color_caps: Default::default(),
            role: crate::output::OutputRole::Extend,
            role_source: crate::output::OutputRoleSource::Auto,
        }
    }

    #[test]
    fn presentation_domain_period_counts_physical_outputs_only() {
        // A virtual output is a headless stream target, never part of the DRM
        // atomic batch, so a slow recording target must not throttle the
        // desktop: 60 Hz + 144 Hz physical, plus a 24 Hz virtual, presents at
        // the slowest *physical* rate (60 Hz), not 24 Hz.
        let outputs = [
            info("eDP-1", 60_000, OutputKind::Internal),
            info("DP-1", 144_000, OutputKind::External),
            info("VIRTUAL-1", 24_000, OutputKind::Virtual),
        ];
        assert_eq!(
            presentation_domain_period_ns(&outputs),
            Some(16_666_666),
            "a 24 Hz virtual output must not drag the physical domain to 24 Hz"
        );

        // Physical-only inputs still resolve through the slowest panel.
        let physical_only = [
            info("DP-1", 165_000, OutputKind::External),
            info("DP-2", 60_000, OutputKind::External),
        ];
        assert_eq!(
            presentation_domain_period_ns(&physical_only),
            Some(16_666_666)
        );

        // No physical output at all (a headless desktop driving only virtual
        // outputs) has no vblank, so the honest answer is "unknown".
        let virtual_only = [info("VIRTUAL-1", 60_000, OutputKind::Virtual)];
        assert_eq!(presentation_domain_period_ns(&virtual_only), None);
        assert_eq!(presentation_domain_period_ns(&[]), None);
    }

    #[test]
    fn identity_scale_and_transform_keeps_physical_size() {
        let g = OutputGeometry {
            mode: mode(1920, 1080),
            logical_size_override: None,
            ..Default::default()
        };
        assert_eq!(g.logical_size(), Size { w: 1920, h: 1080 });
    }

    #[test]
    fn integer_scale_halves_a_hidpi_mode() {
        let g = OutputGeometry {
            mode: mode(3840, 2160),
            scale: Scale(2.0),
            logical_size_override: None,
            ..Default::default()
        };
        assert_eq!(g.logical_size(), Size { w: 1920, h: 1080 });
    }

    #[test]
    fn fractional_scale_supports_non_integer() {
        let g = OutputGeometry {
            mode: mode(3000, 2000),
            scale: Scale(1.5),
            logical_size_override: None,
            ..Default::default()
        };
        // 3000/1.5 = 2000, 2000/1.5 ≈ 1333.33 → 1333.
        assert_eq!(g.logical_size(), Size { w: 2000, h: 1333 });
    }

    #[test]
    fn rotate_90_swaps_axes() {
        let g = OutputGeometry {
            mode: mode(1920, 1080), // landscape panel
            transform: Transform::Rotate90,
            logical_size_override: None,
            ..Default::default()
        };
        // Rotated portrait: logical width = physical height and vice versa.
        assert_eq!(g.logical_size(), Size { w: 1080, h: 1920 });
    }

    #[test]
    fn rotate_90_and_scale_compose() {
        let g = OutputGeometry {
            mode: mode(3840, 2160),
            scale: Scale(2.0),
            transform: Transform::Rotate90,
            logical_size_override: None,
            ..Default::default()
        };
        // Swap → (2160, 3840); /2 → (1080, 1920).
        assert_eq!(g.logical_size(), Size { w: 1080, h: 1920 });
    }

    #[test]
    fn flip_variants_swap_axes_only_for_90_270() {
        let normal = OutputGeometry {
            mode: mode(1920, 1080),
            logical_size_override: None,
            ..Default::default()
        };
        let flip_h = OutputGeometry {
            mode: mode(1920, 1080),
            transform: Transform::FlipHorizontal,
            ..normal
        };
        // Pure flips (no rotation) do not swap axes.
        assert_eq!(flip_h.logical_size(), normal.logical_size());
    }

    #[test]
    fn logical_rect_combines_origin_and_size() {
        let g = OutputGeometry {
            mode: mode(2560, 1440),
            scale: Scale(2.0),
            logical_origin: Point { x: 960, y: 0 },
            logical_size_override: None,
            ..Default::default()
        };
        assert_eq!(
            g.logical_rect(),
            Rect {
                origin: Point { x: 960, y: 0 },
                size: Size { w: 1280, h: 720 },
            }
        );
    }

    #[test]
    fn non_positive_scale_falls_back_to_physical() {
        let g = OutputGeometry {
            mode: mode(100, 50),
            scale: Scale(0.0),
            logical_size_override: None,
            ..Default::default()
        };
        assert_eq!(g.logical_size(), Size { w: 100, h: 50 });
    }

    #[test]
    fn physical_ppi_uses_mode_and_millimeter_dimensions() {
        let ppi = physical_ppi(mode(3072, 1920), (312, 195)).unwrap();
        assert!((ppi - 250.09).abs() < 0.01, "{ppi}");
    }

    #[test]
    fn automatic_scale_selects_two_for_hidpi_laptop_panel() {
        assert_eq!(
            automatic_scale(mode(3072, 1920), (312, 195), OutputKind::Internal),
            Some(Scale(2.0))
        );
    }

    #[test]
    fn automatic_scale_uses_output_kind_and_integral_steps() {
        // A typical 15.6-inch 1080p laptop panel is about 141 PPI: 200% would
        // drop logical density to 71 PPI, below the internal floor.
        assert_eq!(
            automatic_scale(mode(1920, 1080), (344, 194), OutputKind::Internal),
            Some(Scale(1.0))
        );
        // A 27-inch 4K external display is about 163 PPI and is normally
        // viewed farther away, so it steps up to the mainstream 200%.
        assert_eq!(
            automatic_scale(mode(3840, 2160), (598, 336), OutputKind::External),
            Some(Scale(2.0))
        );
        assert_eq!(
            automatic_scale(mode(1920, 1080), (509, 286), OutputKind::External),
            Some(Scale::IDENTITY)
        );
    }

    #[test]
    fn test_inv_arch_73_automatic_scale_stays_on_the_integral_grid() {
        // Sweep a wide PPI range: a fractional recommendation would make every
        // legacy client render at ceil()x the physical grid and would break the
        // mode-extent equality direct scanout depends on.
        for ppi in [60_u32, 90, 109, 130, 141, 163, 190, 220, 250, 282, 340, 440] {
            for kind in [OutputKind::Internal, OutputKind::External] {
                let mode = mode(ppi as i32 * 10, ppi as i32 * 10);
                let mm = 25_400_u64
                    .checked_div(ppi as u64)
                    .map(|edge| (edge as u32, edge as u32))
                    .unwrap();
                let Some(scale) = automatic_scale(mode, mm, kind) else {
                    continue;
                };
                let raw = scale.as_f32();
                assert_eq!(raw, raw.round(), "{ppi} PPI {kind:?} -> {raw}");
                assert!((1.0..=4.0).contains(&raw), "{ppi} PPI {kind:?} -> {raw}");
            }
        }
    }

    #[test]
    fn automatic_scale_rejects_missing_or_inconsistent_physical_size() {
        assert_eq!(
            automatic_scale(mode(3072, 1920), (0, 0), OutputKind::Internal),
            None
        );
        assert_eq!(
            automatic_scale(mode(1920, 1080), (300, 100), OutputKind::Internal),
            None
        );
    }

    #[test]
    fn mode_spec_parses_resolution_with_optional_refresh() {
        assert_eq!(
            "1920x1080".parse::<ModeSpec>(),
            Ok(ModeSpec {
                width: 1920,
                height: 1080,
                refresh_hz: None,
            })
        );
        assert_eq!(
            "2560x1440@144".parse::<ModeSpec>(),
            Ok(ModeSpec {
                width: 2560,
                height: 1440,
                refresh_hz: Some(144),
            })
        );
    }

    #[test]
    fn mode_spec_rejects_garbage() {
        for bad in [
            "",
            "1920",
            "1920x",
            "x1080",
            "1920X1080",
            "1920 x 1080",
            "1920x1080x2",
            "1920x1080@",
            "1920x1080@60@2",
            "0x1080",
            "1920x-1080",
            "+1920x1080",
            "1920x1080@0",
            "99999999999x1080",
        ] {
            assert!(bad.parse::<ModeSpec>().is_err(), "{bad:?} should fail");
        }
    }

    #[test]
    fn mode_spec_matches_exact_size_and_rounded_refresh() {
        let spec: ModeSpec = "1920x1080@144".parse().unwrap();
        assert!(spec.matches(&OutputMode {
            width: 1920,
            height: 1080,
            refresh_mhz: 144_000,
        }));
        // 143.856 Hz rounds to 144.
        assert!(spec.matches(&OutputMode {
            width: 1920,
            height: 1080,
            refresh_mhz: 143_856,
        }));
        // Wrong refresh, wrong size, and 144.5+ Hz (rounds to 145) all miss.
        assert!(!spec.matches(&OutputMode {
            width: 1920,
            height: 1080,
            refresh_mhz: 60_000,
        }));
        assert!(!spec.matches(&OutputMode {
            width: 2560,
            height: 1080,
            refresh_mhz: 144_000,
        }));
        assert!(!spec.matches(&OutputMode {
            width: 1920,
            height: 1080,
            refresh_mhz: 144_501,
        }));
        // No refresh in the spec matches any rate of the right size.
        let any_rate: ModeSpec = "1920x1080".parse().unwrap();
        assert!(any_rate.matches(&OutputMode {
            width: 1920,
            height: 1080,
            refresh_mhz: 60_000,
        }));
    }

    #[test]
    fn virtual_output_spec_validates_connector_dimensions_and_frame_ceiling() {
        let valid = VirtualOutputSpec {
            connector: Some("VIRTUAL-1".into()),
            mode: OutputMode {
                width: 1920,
                height: 1080,
                refresh_mhz: 60_000,
            },
            scale: Scale::IDENTITY,
            logical_origin: Some(Point { x: 1920, y: 0 }),
            persistent: false,
        };
        assert_eq!(valid.validate(), Ok(()));

        // Auto-named virtual connector is valid.
        let auto_named = VirtualOutputSpec {
            connector: None,
            ..valid
        };
        assert_eq!(auto_named.validate(), Ok(()));

        // Non-VIRTUAL prefix or empty suffix is rejected.
        for bad_name in ["HDMI-A-1", "VIRTUAL-", "VIRTUAL-1/bad", "virtual-1"] {
            let spec = VirtualOutputSpec {
                connector: Some(bad_name.into()),
                ..valid
            };
            assert!(spec.validate().is_err(), "{bad_name:?} must fail");
        }

        // Oversized pixel area (> 67,108,864 pixels) is rejected even when
        // individual dimensions are within 64..=16_384.
        let oversized = VirtualOutputSpec {
            mode: OutputMode {
                width: 16_384,
                height: 8_192,
                refresh_mhz: 60_000,
            },
            ..valid
        };
        assert!(oversized.validate().is_err());

        // Virtual outputs always recommend 1.0 scale regardless of millimeter hints.
        assert_eq!(
            automatic_scale(mode(3840, 2160), (0, 0), OutputKind::Virtual),
            Some(Scale::IDENTITY)
        );
    }

    fn role_info(connector: &str, kind: OutputKind, mode: OutputMode) -> OutputInfo {
        OutputInfo {
            connector: connector.into(),
            geometry: OutputGeometry {
                mode,
                scale: Scale::IDENTITY,
                transform: Transform::Normal,
                logical_origin: Point::default(),
                logical_size_override: None,
            },
            kind,
            available_modes: Vec::new(),
            color_caps: tessera_primitives::edid::EdidColorCapabilities::default(),
            role: OutputRole::Extend,
            role_source: crate::output::OutputRoleSource::Auto,
        }
    }

    #[test]
    fn default_topology_is_mirror_and_mirrors_larger_external_hdmi() {
        assert_eq!(DisplayTopology::default(), DisplayTopology::Mirror);
        let outputs = [
            role_info("eDP-1", OutputKind::Internal, mode(3072, 1920)),
            role_info("HDMI-A-1", OutputKind::External, mode(3840, 2160)),
        ];
        let roles = resolve_output_roles(&outputs, &Default::default(), DisplayTopology::default());
        assert_eq!(roles[0], OutputRole::Extend);
        assert_eq!(
            roles[1],
            OutputRole::Mirror {
                source: "eDP-1".into()
            }
        );
    }

    #[test]
    fn auto_topology_mirrors_a_smaller_external_display() {
        let outputs = [
            role_info("eDP-1", OutputKind::Internal, mode(1920, 1080)),
            role_info("HDMI-A-1", OutputKind::External, mode(1280, 720)),
        ];
        let roles = resolve_output_roles(&outputs, &Default::default(), DisplayTopology::Auto);
        assert_eq!(roles[0], OutputRole::Extend);
        assert_eq!(
            roles[1],
            OutputRole::Mirror {
                source: "eDP-1".into()
            }
        );
    }

    #[test]
    fn auto_topology_extends_a_larger_external_display() {
        let outputs = [
            role_info("eDP-1", OutputKind::Internal, mode(1920, 1080)),
            role_info("HDMI-A-1", OutputKind::External, mode(3840, 2160)),
        ];
        let roles = resolve_output_roles(&outputs, &Default::default(), DisplayTopology::Auto);
        assert_eq!(roles[1], OutputRole::Extend);
    }

    #[test]
    fn auto_topology_extends_equal_external_monitors() {
        let outputs = [
            role_info("DP-1", OutputKind::External, mode(2560, 1440)),
            role_info("HDMI-A-1", OutputKind::External, mode(2560, 1440)),
        ];
        let roles = resolve_output_roles(&outputs, &Default::default(), DisplayTopology::Auto);
        assert_eq!(roles[1], OutputRole::Extend);
    }

    #[test]
    fn auto_topology_uses_the_native_mode_list_not_the_selected_mode() {
        let mut source = role_info("DP-1", OutputKind::External, mode(1280, 720));
        source.available_modes = vec![mode(1920, 1080), mode(1280, 720)];
        let outputs = [
            source,
            role_info("HDMI-A-1", OutputKind::External, mode(1600, 900)),
        ];
        let roles = resolve_output_roles(&outputs, &Default::default(), DisplayTopology::Auto);
        assert_eq!(
            roles[1],
            OutputRole::Mirror {
                source: "DP-1".into()
            }
        );
    }

    #[test]
    fn an_explicit_primary_is_the_auto_mirror_source_not_the_internal_panel() {
        let outputs = [
            role_info("eDP-1", OutputKind::Internal, mode(1920, 1080)),
            role_info("HDMI-A-1", OutputKind::External, mode(1920, 1080)),
        ];
        let policies = std::collections::HashMap::from([(
            "HDMI-A-1".to_owned(),
            OutputPolicy {
                primary: true,
                ..Default::default()
            },
        )]);
        let roles = resolve_output_roles(&outputs, &policies, DisplayTopology::Auto);
        assert_eq!(roles[0], OutputRole::Extend);
        assert_eq!(
            roles[1],
            OutputRole::Extend,
            "an explicit primary is the source, and the internal panel extends"
        );
    }

    #[test]
    fn explicit_role_wins_over_topology_policy() {
        let outputs = [
            role_info("eDP-1", OutputKind::Internal, mode(1920, 1080)),
            role_info("HDMI-A-1", OutputKind::External, mode(1280, 720)),
        ];
        let policies = std::collections::HashMap::from([(
            "HDMI-A-1".to_owned(),
            OutputPolicy {
                role: Some(OutputRole::Extend),
                ..Default::default()
            },
        )]);
        let roles = resolve_output_roles(&outputs, &policies, DisplayTopology::Auto);
        assert_eq!(roles[1], OutputRole::Extend);
    }

    #[test]
    fn an_explicit_mirror_source_is_not_re_mirrored_by_the_topology() {
        let outputs = [
            role_info("eDP-1", OutputKind::Internal, mode(1920, 1080)),
            role_info("DP-1", OutputKind::External, mode(2560, 1440)),
            role_info("HDMI-A-1", OutputKind::External, mode(1280, 720)),
        ];
        let policies = std::collections::HashMap::from([
            (
                "eDP-1".to_owned(),
                OutputPolicy {
                    primary: true,
                    ..Default::default()
                },
            ),
            (
                "HDMI-A-1".to_owned(),
                OutputPolicy {
                    role: Some(OutputRole::Mirror {
                        source: "DP-1".into(),
                    }),
                    ..Default::default()
                },
            ),
        ]);
        let roles = resolve_output_roles(&outputs, &policies, DisplayTopology::Mirror);
        assert_eq!(roles[0], OutputRole::Extend);
        assert_eq!(
            roles[1],
            OutputRole::Extend,
            "the explicit source stays extended"
        );
        assert_eq!(
            roles[2],
            OutputRole::Mirror {
                source: "DP-1".into()
            }
        );
    }

    #[test]
    fn dangling_or_chained_mirrors_degrade_to_extend() {
        let outputs = [
            role_info("eDP-1", OutputKind::Internal, mode(1920, 1080)),
            role_info("HDMI-A-1", OutputKind::External, mode(1280, 720)),
            role_info("DP-1", OutputKind::External, mode(1920, 1080)),
        ];
        let policies = std::collections::HashMap::from([
            (
                "HDMI-A-1".to_owned(),
                OutputPolicy {
                    role: Some(OutputRole::Mirror {
                        source: "missing".into(),
                    }),
                    ..Default::default()
                },
            ),
            (
                "DP-1".to_owned(),
                OutputPolicy {
                    role: Some(OutputRole::Mirror {
                        source: "HDMI-A-1".into(),
                    }),
                    ..Default::default()
                },
            ),
        ]);
        let roles = resolve_output_roles(&outputs, &policies, DisplayTopology::Auto);
        assert_eq!(roles[1], OutputRole::Extend);
        assert_eq!(roles[2], OutputRole::Extend);
    }

    fn content_info(role: OutputRole, dims: (i32, i32), logical: (i32, i32)) -> OutputInfo {
        OutputInfo {
            connector: "test".into(),
            geometry: OutputGeometry {
                mode: mode(dims.0, dims.1),
                scale: Scale::IDENTITY,
                transform: Transform::Normal,
                logical_origin: Point::default(),
                logical_size_override: Some(Size {
                    w: logical.0,
                    h: logical.1,
                }),
            },
            kind: OutputKind::External,
            available_modes: Vec::new(),
            color_caps: tessera_primitives::edid::EdidColorCapabilities::default(),
            role,
            role_source: OutputRoleSource::Explicit,
        }
    }

    #[test]
    fn extended_output_content_fills_the_panel() {
        let output = content_info(OutputRole::Extend, (1920, 1080), (1920, 1080));
        assert_eq!(output.content_fraction(), (0.0, 0.0, 1.0, 1.0));
    }

    #[test]
    fn wider_mirror_panel_gets_pillarbox_insets() {
        // 16:10 source on a 16:9 panel: 0.9 of the width, centered.
        let output = content_info(
            OutputRole::Mirror {
                source: "eDP-1".into(),
            },
            (1920, 1080),
            (2560, 1600),
        );
        let (x, y, w, h) = output.content_fraction();
        assert!((x - 0.05).abs() < 1e-5);
        assert_eq!(y, 0.0);
        assert!((w - 0.9).abs() < 1e-5);
        assert_eq!(h, 1.0);
    }

    #[test]
    fn narrower_mirror_panel_gets_letterbox_insets() {
        // 16:9 source on a 5:4 panel: 0.703125 of the height, centered.
        let output = content_info(
            OutputRole::Mirror {
                source: "eDP-1".into(),
            },
            (1280, 1024),
            (1920, 1080),
        );
        let (x, y, w, h) = output.content_fraction();
        assert_eq!(x, 0.0);
        assert!((y - 0.1484375).abs() < 1e-5);
        assert_eq!(w, 1.0);
        assert!((h - 0.703125).abs() < 1e-5);
    }

    fn group_info(connector: &str, refresh: u32, x: i32, role: OutputRole) -> OutputInfo {
        OutputInfo {
            connector: connector.into(),
            geometry: OutputGeometry {
                mode: OutputMode {
                    width: 1920,
                    height: 1080,
                    refresh_mhz: refresh,
                },
                scale: Scale::IDENTITY,
                transform: Transform::Normal,
                logical_origin: Point { x, y: 0 },
                logical_size_override: None,
            },
            kind: OutputKind::External,
            available_modes: Vec::new(),
            color_caps: tessera_primitives::edid::EdidColorCapabilities::default(),
            role,
            role_source: OutputRoleSource::Explicit,
        }
    }

    #[test]
    fn presentation_groups_join_touching_same_refresh_outputs() {
        let outputs = vec![
            group_info("eDP-1", 60_000, 0, OutputRole::Extend),
            group_info("HDMI-A-1", 60_000, 1920, OutputRole::Extend),
            group_info("DP-1", 60_000, 5000, OutputRole::Extend),
        ];
        let groups = presentation_groups(&outputs);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].outputs, ["eDP-1", "HDMI-A-1"]);
        assert_eq!(groups[1].outputs, ["DP-1"]);
    }

    #[test]
    fn presentation_groups_split_mixed_refresh_neighbors() {
        let outputs = vec![
            group_info("eDP-1", 165_000, 0, OutputRole::Extend),
            group_info("HDMI-A-1", 60_000, 1920, OutputRole::Extend),
        ];
        let groups = presentation_groups(&outputs);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].refresh_mhz, 165_000);
        assert_eq!(groups[1].refresh_mhz, 60_000);
    }

    #[test]
    fn presentation_groups_merge_for_a_window_spanning_groups() {
        let outputs = vec![
            group_info("eDP-1", 165_000, 0, OutputRole::Extend),
            group_info("HDMI-A-1", 60_000, 1920, OutputRole::Extend),
        ];
        assert_eq!(presentation_groups(&outputs).len(), 2);
        let mut window = crate::window::Window::new(crate::window::WindowId(1));
        window.position = tessera_primitives::Point { x: 1500, y: 100 };
        window.size = tessera_primitives::Size { w: 800, h: 600 };
        let groups = presentation_groups_for_windows(&outputs, &[window]);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].outputs, ["eDP-1", "HDMI-A-1"]);
        assert_eq!(groups[0].refresh_mhz, 60_000);
    }

    #[test]
    fn presentation_groups_follow_mirror_source() {
        let outputs = vec![
            group_info("eDP-1", 165_000, 0, OutputRole::Extend),
            group_info(
                "HDMI-A-1",
                60_000,
                0,
                OutputRole::Mirror {
                    source: "eDP-1".into(),
                },
            ),
        ];
        let groups = presentation_groups(&outputs);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].outputs, ["eDP-1", "HDMI-A-1"]);
        assert_eq!(groups[0].refresh_mhz, 60_000);
    }

    #[test]
    fn mirror_geometry_uses_the_source_logical_extent() {
        let mut geometry = OutputGeometry {
            mode: mode(1920, 1080),
            scale: Scale(1.5),
            transform: Transform::Normal,
            logical_origin: Point { x: 320, y: 10 },
            logical_size_override: Some(Size { w: 1280, h: 720 }),
        };
        assert_eq!(geometry.logical_size(), Size { w: 1280, h: 720 });
        assert_eq!(geometry.logical_rect(), Rect::new(320, 10, 1280, 720));
        geometry.logical_size_override = None;
        assert_eq!(geometry.logical_size(), Size { w: 1280, h: 720 });
    }
}
