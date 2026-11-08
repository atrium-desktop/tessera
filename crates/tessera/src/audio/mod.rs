//! Audio subsystem: the sound server's own truth about outputs and inputs.
//!
//! Tessera does not mix, resample, or route audio. PipeWire (with WirePlumber's
//! policy on top) owns the graph, the device inventory, the default nodes, and
//! every stream's links; this subsystem's whole job is to **observe** that graph
//! and to forward a user's intent back to it as a *named* request.
//!
//! # Why this is a module and not a crate
//!
//! ADR-0159 (package boundaries follow capabilities) retains a package only for
//! a concrete dependency, build, or delivery boundary. Audio has none: it links
//! no client library, delivers no binary, and its only platform interaction is
//! short-lived CLI probes through `crate::host_command`. The dependency
//! firewall would gain a manifest and lose nothing. The shape here therefore
//! mirrors `wireless` and `bluetooth` — a `Backend` trait, one parse seam that
//! is testable against a captured document, a mock for deterministic tests and
//! preview, and a strict boundary between *observation* and *intent* — without
//! paying for a fourth manifest.
//!
//! # The two halves, and why they are asymmetric (ADR-0215, ADR-0216)
//!
//! Outputs and inputs are structurally identical in PipeWire (`Audio/Sink` and
//! `Audio/Source` nodes, `default.audio.sink` and `default.audio.source`
//! metadata) but they carry different *risk*:
//!
//! - A **default output** that is wrong plays music in the wrong room. The cost
//!   is social, and it is fixed by one more click.
//! - A **default input** that is wrong captures the wrong microphone. The cost
//!   is a privacy leak, and a *muted* microphone that the desktop forgot it
//!   muted is a silent promise it cannot keep.
//!
//! So the two halves share the handle discipline (`[INV-AUDIO-1]`: the server's
//! stable `node.name`, never a node number) and differ where the risk does: the
//! input half additionally reports *whether any client is capturing right now*
//! (`microphone_active`), observed from the server's live link graph rather than
//! inferred from chrome's own bookkeeping. `[INV-AUDIO-7]`.
//!
//! # What this module refuses to do
//!
//! - It does not remember a "preferred" device. The server is the only
//!   authority; a stored preference the server never reads is a UI that lies
//!   (`[INV-AUDIO-3]`).
//! - It does not cache a click optimistically. Routing changes are confirmed by
//!   the next probe, because `pw-metadata` exits 0 whether or not the name
//!   resolved (`[INV-AUDIO-2]`).
//! - It does not classify transports from presentation strings. Classification
//!   happens here, once, from the server's own vocabulary (`[INV-AUDIO-4]`).

use serde_json::Value;
use tessera_desktop::system::{AudioInput, AudioInputKind, AudioOutput, AudioOutputKind};

use crate::host_command::{command_output, spawn_host_command};

#[cfg(test)]
mod tests;

/// The `default` metadata object's key for the live default playback node.
pub(crate) const DEFAULT_SINK_KEY: &str = "default.audio.sink";
/// The `default` metadata object's key for the live default capture node.
pub(crate) const DEFAULT_SOURCE_KEY: &str = "default.audio.source";
/// The key WirePlumber persists a user's chosen sink under, and re-asserts.
pub(crate) const CONFIGURED_SINK_KEY: &str = "default.configured.audio.sink";
/// The persisted-preference counterpart for capture.
pub(crate) const CONFIGURED_SOURCE_KEY: &str = "default.configured.audio.source";

/// One coherent observation of the sound server's audio graph.
///
/// Everything here comes from **one** `pw-dump` and **one** `pw-metadata` read
/// per full probe, so the two halves, the default markers, and the capture flag
/// can never describe different moments of an audio graph that is changing
/// under the desktop. `[INV-AUDIO-6]`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct AudioSnapshot {
    /// Playback sinks the server exposes, ordered for presentation.
    pub outputs: Vec<AudioOutput>,
    /// Capture sources the server exposes, ordered for presentation.
    pub inputs: Vec<AudioInput>,
    /// The live default sink's volume in percent, or `None` when no server
    /// answered. `None` is "unknown", never "silent".
    pub output_volume: Option<u8>,
    /// Whether the live default sink is muted.
    pub output_muted: bool,
    /// The live default source's capture volume in percent.
    pub input_volume: Option<u8>,
    /// Whether the live default source is muted.
    pub input_muted: bool,
    /// Whether a client is capturing audio from any source right now.
    pub capturing: bool,
}

impl AudioSnapshot {
    /// An empty observation: no sound server answered.
    ///
    /// This is the honest value for a host without PipeWire, and presentation
    /// reads it as "no audio graph", never as "no hardware" (`[INV-AUDIO-5]`).
    pub(crate) fn absent() -> Self {
        Self::default()
    }
}

/// The daemon-neutral contract for observing and steering the sound server.
///
/// One trait, implemented twice (a live PipeWire CLI bridge and a scripted
/// mock), so the compositor never names PipeWire outside this module and every
/// test drives the same contract the shipping path does.
pub(crate) trait AudioBackend: Send + Sync + std::fmt::Debug {
    /// Observe the graph as of now. Called from the status poller thread only.
    fn snapshot(&self) -> AudioSnapshot;

    /// Route new playback to `name` (the server's node name).
    fn set_output(&self, name: &str) -> Result<(), String>;

    /// Route new capture streams to `name` (the server's node name).
    fn set_input(&self, name: &str) -> Result<(), String>;

    /// Set the live default sink's volume percentage.
    fn set_output_volume(&self, level: u8) -> Result<(), String>;

    /// Step the live default sink's volume by `delta` percentage points.
    fn step_output_volume(&self, delta: i8) -> Result<(), String>;

    /// Toggle the live default sink's mute.
    fn toggle_output_mute(&self) -> Result<(), String>;

    /// Set the live default source's capture volume percentage.
    fn set_input_volume(&self, level: u8) -> Result<(), String>;

    /// Toggle the live default source's mute.
    fn toggle_input_mute(&self) -> Result<(), String>;
}

// ---------------------------------------------------------------------------
// Live backend: PipeWire + WirePlumber through their own CLI tools.
// ---------------------------------------------------------------------------

/// The shipping backend: `pw-dump`/`pw-metadata`/`wpctl` against the live server.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct PipeWireBackend;

impl PipeWireBackend {
    pub(crate) fn new() -> Self {
        Self
    }
}

impl AudioBackend for PipeWireBackend {
    fn snapshot(&self) -> AudioSnapshot {
        // One process per half. `pw-dump` already carries the node list, the
        // per-node volume/mute, the `default` metadata object, and the live
        // link graph; asking `wpctl get-volume` as well would fork twice for
        // one number and could disagree with the dump it sits beside.
        let dump = command_output("pw-dump", &[]).and_then(as_document);
        let Some(objects) = dump else {
            return AudioSnapshot::absent();
        };
        let live = LiveGraph::of(&objects);
        live.snapshot()
    }

    fn set_output(&self, name: &str) -> Result<(), String> {
        write_default(CONFIGURED_SINK_KEY, DEFAULT_SINK_KEY, name)
    }

    fn set_input(&self, name: &str) -> Result<(), String> {
        write_default(CONFIGURED_SOURCE_KEY, DEFAULT_SOURCE_KEY, name)
    }

    fn set_output_volume(&self, level: u8) -> Result<(), String> {
        set_default_volume("@DEFAULT_AUDIO_SINK@", level)
    }

    fn step_output_volume(&self, delta: i8) -> Result<(), String> {
        step_default_volume("@DEFAULT_AUDIO_SINK@", delta)
    }

    fn toggle_output_mute(&self) -> Result<(), String> {
        spawn_host_command("wpctl", &["set-mute", "@DEFAULT_AUDIO_SINK@", "toggle"])
    }

    fn set_input_volume(&self, level: u8) -> Result<(), String> {
        set_default_volume("@DEFAULT_AUDIO_SOURCE@", level)
    }

    fn toggle_input_mute(&self) -> Result<(), String> {
        spawn_host_command("wpctl", &["set-mute", "@DEFAULT_AUDIO_SOURCE@", "toggle"])
    }
}

/// Write one name into the server's `default` metadata object, in **both** the
/// persisted and the live key.
///
/// Two keys, because they are two different statements and the server treats
/// them differently:
///
/// - `default.configured.<kind>` is the *stored preference*: the key
///   `wpctl set-default` writes and the one WirePlumber restores from after a
///   restart or a device re-plug.
/// - `default.audio.<kind>` is the *live default*: what new and followed streams
///   attach to right now, and what the panel's default marker reads.
///
/// ADR-0215 wrote only the first, on the belief that WirePlumber re-elects the
/// live default from it. Measured against WirePlumber 0.5.14 that is false: a
/// `default.configured.audio.sink` write left the live key **and a running
/// `pw-play` link** exactly where they were for ten seconds, and `wpctl
/// set-default <id>` behaved identically. A switch made through the shipping
/// path would therefore have moved nothing, with no error to explain it — so
/// both keys are written.
///
/// The order is preference-then-live in the loop, but neither write waits for
/// the other: both are fire-and-forget spawns. That is deliberate rather than
/// sloppy. The ordering carries no correctness requirement — the `configured`
/// write is inert on its own (measured), so a re-election cannot observe a live
/// key that disagrees with a stale preference — and a spawn that blocked would
/// put a `pw-metadata` round trip on the compositor's dispatch path. What *is*
/// required is that neither write is skipped, which the loop guarantees and
/// `a_routing_write_covers_both_the_preference_and_the_live_default` pins.
///
/// The write is *name*-addressed end to end: the numeric node id is reassigned
/// on every server restart, so resolving it first would add a lookup that can go
/// stale between the two calls (`[INV-AUDIO-1]`).
fn write_default(configured_key: &str, live_key: &str, name: &str) -> Result<(), String> {
    let value = serde_json::json!({ "name": name }).to_string();
    for key in [configured_key, live_key] {
        spawn_host_command(
            "pw-metadata",
            &["-n", "default", "0", key, &value, "Spa:String:JSON"],
        )?;
    }
    Ok(())
}

/// Set the live default node's volume, and end a mute in the same request.
///
/// **The mute is cleared explicitly, because the server does not do it.**
/// Measured against WirePlumber 0.5.14: `wpctl set-volume` on a muted sink
/// leaves it `[MUTED]`, for the sink and the source alike. A desktop that
/// assumed otherwise would raise the level, claim the mute was gone, and leave
/// the user dragging a fader that produces no sound.
///
/// Unmuting here (and only here — see [`step_default_volume`]) is the deliberate
/// half of the pair: dragging a level control is a statement about *this value*
/// with the mute glyph in view, which is what every other desktop does. A
/// keyboard nudge is ambient and can be accidental, so it must not silently undo
/// a mute the user chose.
///
/// `-l 1.0` pins the node's volume limit, so a stored preference cannot keep the
/// fader from reaching 100% after a device re-plug.
fn set_default_volume(target: &str, level: u8) -> Result<(), String> {
    let amount = format!("{level}%");
    spawn_host_command("wpctl", &["set-volume", target, &amount, "-l", "1.0"])?;
    spawn_host_command("wpctl", &["set-mute", target, "0"])
}

/// Step the live default node's volume by a signed percentage.
///
/// Deliberately does **not** clear a mute, matching what the server does on its
/// own and diverging from [`set_default_volume`] on purpose. A keyboard nudge is
/// ambient — it can be pressed by accident, in a meeting, on a machine someone
/// muted deliberately — so it changes the level and leaves the mute alone. The
/// user hears nothing, sees the muted glyph beside the fader, and knows exactly
/// why.
///
/// The `-l 1.0` limit is carried here too, so a stepped fader cannot climb past
/// full scale on a node whose stored limit is higher.
fn step_default_volume(target: &str, delta: i8) -> Result<(), String> {
    let amount = if delta >= 0 {
        format!("{delta}%+")
    } else {
        format!("{}%-", delta.unsigned_abs())
    };
    spawn_host_command("wpctl", &["set-volume", target, &amount, "-l", "1.0"])
}

/// Interpret a `pw-dump` document, or reject it.
///
/// A document that does not parse is not an empty graph with confidence: it is
/// no observation at all, and the caller reports "no sound server answered"
/// rather than an inventory that happens to be empty for the wrong reason.
fn as_document(dump: String) -> Option<Vec<Value>> {
    match serde_json::from_str::<Value>(&dump).ok()? {
        Value::Array(objects) => Some(objects),
        _ => None,
    }
}

/// A borrowed view over one `pw-dump` document.
///
/// Splitting the parse out of the fork is what makes the version-sensitive
/// shapes testable: the fixtures in `tests.rs` are captured documents, not the
/// machine the suite happens to run on.
struct LiveGraph<'a> {
    objects: &'a [Value],
}

impl<'a> LiveGraph<'a> {
    fn of(objects: &'a [Value]) -> Self {
        Self { objects }
    }

    /// Every node whose `media.class` is `wanted`.
    ///
    /// A node is admitted whether or not it publishes a `node.name`: whether the
    /// node is *addressable* is a question for the inventory builders, while
    /// capture detection only needs its identity. Conflating the two would let an
    /// unnamed source be recorded from without being reported.
    fn nodes<'b>(&'b self, wanted: &'b str) -> impl Iterator<Item = Node<'a>> + 'b {
        self.objects
            .iter()
            .filter(move |object| {
                object.get("type").and_then(Value::as_str) == Some("PipeWire:Interface:Node")
            })
            .filter_map(move |object| Node::of(object, wanted))
    }

    /// The node name the `default` metadata object currently routes to `key`.
    fn default_name(&self, key: &str) -> Option<String> {
        self.objects
            .iter()
            .filter(|object| {
                object.get("type").and_then(Value::as_str) == Some("PipeWire:Interface:Metadata")
            })
            .filter(|object| {
                object
                    .get("props")
                    .and_then(|props| props.get("metadata.name"))
                    .and_then(Value::as_str)
                    == Some("default")
            })
            .find_map(|object| {
                object
                    .get("metadata")?
                    .as_array()?
                    .iter()
                    .filter(|entry| entry.get("key").and_then(Value::as_str) == Some(key))
                    .find_map(|entry| metadata_name(entry.get("value")?))
            })
    }

    /// Whether a live link feeds any capture stream from an `Audio/Source`.
    ///
    /// Read from the server's link graph, not from chrome: a browser tab, an
    /// encoder, and an agent session are indistinguishable here, which is
    /// exactly right — the indicator's job is to report that the machine is
    /// recording, whoever asked (`[INV-AUDIO-7]`).
    ///
    /// A source node's *monitor* ports are not a capture: monitoring is what
    /// the desktop does to its own playback. Only a link whose output end is a
    /// real `Audio/Source` node counts, and a stream that merely exists without
    /// a live link is not recording either — the link is the observation.
    fn capturing(&self) -> bool {
        let sources: Vec<u64> = self
            .nodes("Audio/Source")
            .filter_map(|node| node.id)
            .collect();
        self.objects
            .iter()
            .filter(|object| {
                object.get("type").and_then(Value::as_str) == Some("PipeWire:Interface:Link")
            })
            .filter_map(|object| {
                object
                    .get("info")
                    .and_then(|info| info.get("output-node-id"))
            })
            .filter_map(Value::as_u64)
            .any(|id| sources.contains(&id))
    }

    fn snapshot(&self) -> AudioSnapshot {
        let default_sink = self.default_name(DEFAULT_SINK_KEY);
        let default_source = self.default_name(DEFAULT_SOURCE_KEY);

        let outputs = sorted_outputs(self.nodes("Audio/Sink"), default_sink.as_deref());
        let inputs = sorted_inputs(self.nodes("Audio/Source"), default_source.as_deref());

        // Volume and mute are read off the *node*, because that is where the
        // server keeps them, and both are reported as absent when the default
        // node is missing rather than as a fabricated zero.
        let (output_volume, output_muted) = outputs
            .iter()
            .position(|output| output.is_default)
            .and_then(|index| self.default_node_volume("Audio/Sink", &outputs[index].name))
            .unwrap_or((None, false));
        let (input_volume, input_muted) = inputs
            .iter()
            .position(|input| input.is_default)
            .and_then(|index| self.default_node_volume("Audio/Source", &inputs[index].name))
            .unwrap_or((None, false));

        AudioSnapshot {
            outputs,
            inputs,
            output_volume,
            output_muted,
            input_volume,
            input_muted,
            capturing: self.capturing(),
        }
    }

    /// The `(volume%, muted)` pair of the named node, if the document has it.
    fn default_node_volume(&self, media_class: &str, name: &str) -> Option<(Option<u8>, bool)> {
        let node = self
            .nodes(media_class)
            .find(|node| node.name == Some(name))?;
        node.volume()
    }
}

/// One `Audio/Sink` or `Audio/Source` node from a `pw-dump` document.
struct Node<'a> {
    id: Option<u64>,
    name: Option<&'a str>,
    description: Option<&'a str>,
    api: Option<&'a str>,
    bus: Option<&'a str>,
    info: &'a Value,
}

impl<'a> Node<'a> {
    fn of(object: &'a Value, wanted: &str) -> Option<Self> {
        let info = object.get("info")?;
        let props = info.get("props")?;
        if props.get("media.class").and_then(Value::as_str) != Some(wanted) {
            return None;
        }
        // A node without a `node.name` has no handle a user or a script could
        // keep, so it is not part of the inventory. Capture detection is
        // deliberately separate (`LiveGraph::capturing`): an unnamed source can
        // still be recorded from, and must still be reported.
        Some(Self {
            id: object.get("id").and_then(Value::as_u64),
            name: props.get("node.name").and_then(Value::as_str),
            description: props
                .get("node.description")
                .and_then(Value::as_str)
                .or_else(|| props.get("node.nick").and_then(Value::as_str)),
            api: props.get("device.api").and_then(Value::as_str),
            bus: props.get("device.bus").and_then(Value::as_str),
            info,
        })
    }

    /// The node's `(volume%, muted)` from its `Props` param.
    ///
    /// `pw-dump` reports `Props` as a list whose first entry is the node's own
    /// volume state; the parameter also carries the adapter's entry, so taking
    /// the first is deliberate rather than incidental. A node that reports no
    /// volume (a virtual or a filter node) yields `None`, not zero.
    fn volume(&self) -> Option<(Option<u8>, bool)> {
        let props = self.info.get("params")?.get("Props")?.as_array()?;
        let entry = props.first()?;
        let muted = entry.get("mute").and_then(Value::as_bool).unwrap_or(false);
        // `channelVolumes` is the per-channel cubing of the fader position and
        // `volume` is the node's overall gain; presentation wants the number the
        // user set, which is the channel value the server reports back.
        let channels = entry.get("channelVolumes")?.as_array()?;
        let peak = channels
            .iter()
            .filter_map(Value::as_f64)
            .fold(0.0_f64, f64::max);
        // PipeWire stores cubed amplitude: a fader at 50% is 0.5³ ≈ 0.125. The
        // cube root recovers the position the user actually set, so a reported
        // 65% does not read back as 27%.
        let position = peak.cbrt().clamp(0.0, 1.0);
        Some((Some((position * 100.0).round() as u8), muted))
    }
}

/// The node name a `default` metadata value carries.
///
/// `pw-dump` reports a `Spa:String:JSON` property either as a nested object or
/// as the raw JSON text depending on the server version. Accepting both is not
/// defensive padding: dropping one shape silently loses the default marker and
/// leaves the picker with nothing marked.
fn metadata_name(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => {
            let parsed = serde_json::from_str::<Value>(text).ok()?;
            parsed.get("name")?.as_str().map(str::to_owned)
        }
        other => other.get("name")?.as_str().map(str::to_owned),
    }
}

/// Build the ordered output inventory from the document's sink nodes.
fn sorted_outputs<'a>(
    nodes: impl Iterator<Item = Node<'a>>,
    default: Option<&str>,
) -> Vec<AudioOutput> {
    let mut outputs: Vec<AudioOutput> = nodes
        .filter_map(|node| {
            let name = node.name?.to_owned();
            let description = node.description.unwrap_or(&name).to_owned();
            let kind = classify_output(&name, &description, node.api, node.bus);
            Some(AudioOutput {
                is_default: default == Some(name.as_str()),
                name,
                description,
                kind,
            })
        })
        .collect();
    outputs.sort_by(|left, right| {
        order(left.kind.sort_rank(), left.is_default, &left.name).cmp(&order(
            right.kind.sort_rank(),
            right.is_default,
            &right.name,
        ))
    });
    outputs
}

/// Build the ordered input inventory from the document's source nodes.
fn sorted_inputs<'a>(
    nodes: impl Iterator<Item = Node<'a>>,
    default: Option<&str>,
) -> Vec<AudioInput> {
    let mut inputs: Vec<AudioInput> = nodes
        .filter_map(|node| {
            let name = node.name?.to_owned();
            let description = node.description.unwrap_or(&name).to_owned();
            let kind = classify_input(&name, &description, node.api, node.bus);
            Some(AudioInput {
                is_default: default == Some(name.as_str()),
                name,
                description,
                kind,
            })
        })
        .collect();
    inputs.sort_by(|left, right| {
        order(left.kind.sort_rank(), left.is_default, &left.name).cmp(&order(
            right.kind.sort_rank(),
            right.is_default,
            &right.name,
        ))
    });
    inputs
}

/// The presentation ordering key: transport rank, then the default, then name.
///
/// Grouping is a presentation concern, so the order is decided once here rather
/// than in every painter, and it is decided from stable fields so the list does
/// not reshuffle when the server renumbers its nodes between probes.
fn order(rank: u8, is_default: bool, name: &str) -> (u8, bool, &str) {
    (rank, !is_default, name)
}

/// Map one PipeWire sink node onto the daemon-neutral transport vocabulary.
///
/// `device.api` is the server's own statement about the backing transport — it
/// names the plugin that created the node — and the node name and description
/// are consulted only for analog devices, where "which jack" lives in the label
/// rather than in an API. Anything unrecognized stays
/// [`AudioOutputKind::Other`] instead of being guessed into a category a user
/// would act on (`[INV-AUDIO-4]`).
fn classify_output(
    name: &str,
    description: &str,
    api: Option<&str>,
    bus: Option<&str>,
) -> AudioOutputKind {
    match api {
        Some("bluez5") => return AudioOutputKind::Bluetooth,
        Some("alsa") => {}
        _ => return AudioOutputKind::Other,
    }
    // A USB sound card is still an ALSA node; the bus is what separates it from
    // the machine's own codec.
    if is_usb(bus, name) {
        return AudioOutputKind::Usb;
    }
    let haystack = haystack(name, description);
    if haystack.contains("hdmi") || haystack.contains("displayport") {
        return AudioOutputKind::Hdmi;
    }
    if haystack.contains("headphone")
        || haystack.contains("headset")
        || haystack.contains("lineout")
        || haystack.contains("line-out")
    {
        return AudioOutputKind::Wired;
    }
    if haystack.contains("speaker") {
        return AudioOutputKind::Speaker;
    }
    AudioOutputKind::Other
}

/// Map one PipeWire source node onto the capture-side transport vocabulary
/// (ADR-0216).
///
/// Deliberately *not* a rename of [`classify_output`]: HDMI and "speaker" are
/// meaningless for a capture device, while "digital microphone", "headset mic",
/// and "webcam" are the labels a user actually meets. A camera's audio node is
/// a USB microphone and is classified as one, because that is what it is.
fn classify_input(
    name: &str,
    description: &str,
    api: Option<&str>,
    bus: Option<&str>,
) -> AudioInputKind {
    match api {
        Some("bluez5") => return AudioInputKind::Bluetooth,
        Some("alsa") | Some("v4l2") => {}
        _ => return AudioInputKind::Other,
    }
    if is_usb(bus, name) {
        return AudioInputKind::Usb;
    }
    let haystack = haystack(name, description);
    // A headset jack and a plug-in microphone both carry the word "mic"; the
    // distinction that matters for presentation is whether the device belongs
    // to the machine or hangs off it, and a wired headset is the latter.
    if haystack.contains("headset") || haystack.contains("headphone") {
        return AudioInputKind::Headset;
    }
    if haystack.contains("microphone") || haystack.contains("mic") {
        return AudioInputKind::BuiltIn;
    }
    AudioInputKind::Other
}

/// Whether a node sits on the USB bus, by bus report or by name.
fn is_usb(bus: Option<&str>, name: &str) -> bool {
    matches!(bus, Some("usb")) || name.contains("usb")
}

/// The lowercased `name + description` haystack transport heuristics read.
fn haystack(name: &str, description: &str) -> String {
    format!(
        "{} {}",
        name.to_ascii_lowercase(),
        description.to_ascii_lowercase()
    )
}

// ---------------------------------------------------------------------------
// Mock backend: deterministic observation and mutation for tests and preview.
// ---------------------------------------------------------------------------

/// A scripted sound server for tests, headless CI, and `TESSERA_PREVIEW`.
///
/// It holds the same contract as the live backend — an unknown name is refused,
/// exactly one node is default at a time, a toggle flips the reported state —
/// because a mock that accepts what the real server rejects lets a test pass on
/// behaviour the shipping path does not have.
#[derive(Debug, Clone)]
pub(crate) struct MockAudioBackend {
    state: std::sync::Arc<std::sync::Mutex<MockAudioState>>,
}

/// The mock's observable state.
#[derive(Debug, Clone)]
pub(crate) struct MockAudioState {
    pub outputs: Vec<AudioOutput>,
    pub inputs: Vec<AudioInput>,
    pub output_volume: Option<u8>,
    pub output_muted: bool,
    pub input_volume: Option<u8>,
    pub input_muted: bool,
    pub capturing: bool,
}

impl Default for MockAudioState {
    fn default() -> Self {
        // Two outputs of different transports and two inputs, so preview and the
        // tests exercise the pickers' grouping and default marker instead of a
        // degenerate single-row list.
        Self {
            outputs: vec![
                AudioOutput {
                    name: "mock_output_speaker".into(),
                    description: "Built-in Audio Speaker".into(),
                    kind: AudioOutputKind::Speaker,
                    is_default: true,
                },
                AudioOutput {
                    name: "mock_output_headphones".into(),
                    description: "Built-in Audio Headphones".into(),
                    kind: AudioOutputKind::Wired,
                    is_default: false,
                },
            ],
            inputs: vec![
                AudioInput {
                    name: "mock_input_microphone".into(),
                    description: "Built-in Microphone".into(),
                    kind: AudioInputKind::BuiltIn,
                    is_default: true,
                },
                AudioInput {
                    name: "mock_input_headset".into(),
                    description: "Headset Microphone".into(),
                    kind: AudioInputKind::Headset,
                    is_default: false,
                },
            ],
            output_volume: Some(65),
            output_muted: false,
            input_volume: Some(80),
            input_muted: false,
            capturing: false,
        }
    }
}

impl MockAudioBackend {
    pub(crate) fn new() -> Self {
        Self::with_state(MockAudioState::default())
    }

    pub(crate) fn with_state(state: MockAudioState) -> Self {
        Self {
            state: std::sync::Arc::new(std::sync::Mutex::new(state)),
        }
    }

    /// Shared handle to the mock's state, so a test can assert on what an action
    /// actually did instead of inferring it from the next snapshot.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn state(&self) -> std::sync::Arc<std::sync::Mutex<MockAudioState>> {
        self.state.clone()
    }

    fn update<T>(&self, edit: impl FnOnce(&mut MockAudioState) -> T) -> T {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        edit(&mut state)
    }
}

impl AudioBackend for MockAudioBackend {
    fn snapshot(&self) -> AudioSnapshot {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        AudioSnapshot {
            outputs: state.outputs.clone(),
            inputs: state.inputs.clone(),
            output_volume: state.output_volume,
            output_muted: state.output_muted,
            input_volume: state.input_volume,
            input_muted: state.input_muted,
            capturing: state.capturing,
        }
    }

    fn set_output(&self, name: &str) -> Result<(), String> {
        self.update(|state| {
            if !state.outputs.iter().any(|output| output.name == name) {
                return Err(format!("no audio output named {name}"));
            }
            for output in &mut state.outputs {
                output.is_default = output.name == name;
            }
            Ok(())
        })
    }

    fn set_input(&self, name: &str) -> Result<(), String> {
        self.update(|state| {
            if !state.inputs.iter().any(|input| input.name == name) {
                return Err(format!("no audio input named {name}"));
            }
            for input in &mut state.inputs {
                input.is_default = input.name == name;
            }
            Ok(())
        })
    }

    fn set_output_volume(&self, level: u8) -> Result<(), String> {
        self.update(|state| {
            state.output_volume = Some(level);
            // A level write ends a mute. This mirrors the *whole* backend
            // contract rather than the server's raw behaviour: the live backend
            // issues a second `set-mute 0` because WirePlumber leaves the mute
            // on. A mock that modelled only the raw server would let a runtime
            // assumption pass here and fail on the machine.
            state.output_muted = false;
            Ok(())
        })
    }

    fn step_output_volume(&self, delta: i8) -> Result<(), String> {
        self.update(|state| {
            let current = state.output_volume.unwrap_or(0) as i16;
            // A step does *not* clear the mute: the backend deliberately leaves
            // it, so a keyboard nudge cannot silently undo a chosen mute.
            state.output_volume = Some((current + i16::from(delta)).clamp(0, 100) as u8);
            Ok(())
        })
    }

    fn toggle_output_mute(&self) -> Result<(), String> {
        self.update(|state| {
            state.output_muted = !state.output_muted;
            Ok(())
        })
    }

    fn set_input_volume(&self, level: u8) -> Result<(), String> {
        self.update(|state| {
            state.input_volume = Some(level);
            // Same contract as the sink: the backend clears it, so the mock
            // must too (ADR-0216).
            state.input_muted = false;
            Ok(())
        })
    }

    fn toggle_input_mute(&self) -> Result<(), String> {
        self.update(|state| {
            state.input_muted = !state.input_muted;
            Ok(())
        })
    }
}
