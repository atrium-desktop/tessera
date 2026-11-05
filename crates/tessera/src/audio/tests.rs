//! Tests for the audio subsystem (ADR-0215, ADR-0216).
//!
//! The parse is split from the fork so these fixtures are *captured documents*
//! — trimmed `pw-dump` output from a live server — rather than the machine the
//! suite happens to run on. Every assertion below is about a shape a real
//! server emits, including the ones that only differ by version.

use super::*;
use serde_json::json;

/// One `pw-dump` document with the metadata object and a mixed node set.
///
/// The source nodes are present on purpose: a sink and a source share the node
/// shape and differ only by `media.class`, so a filter that keys on "is an audio
/// node" would put a microphone in the output picker.
fn document() -> Vec<Value> {
    let text = r#"[
      {
        "id": 39,
        "type": "PipeWire:Interface:Metadata",
        "props": { "metadata.name": "default" },
        "metadata": [
          { "subject": 0, "key": "default.audio.sink",
            "value": { "name": "bluez_output.AA.1" }, "type": "Spa:String:JSON" },
          { "subject": 0, "key": "default.audio.source",
            "value": "{\"name\":\"alsa_input.headset\"}", "type": "Spa:String:JSON" }
        ]
      },
      { "id": 61, "type": "PipeWire:Interface:Node", "info": { "props": {
          "media.class": "Audio/Sink",
          "node.name": "alsa_output.hdmi1",
          "node.description": "Arrow Lake cAVS HDMI / DisplayPort 1 Output",
          "device.api": "alsa", "device.bus": "pci" },
        "params": { "Props": [ { "volume": 1.0, "mute": false,
                                 "channelVolumes": [1.0, 1.0] } ] } } },
      { "id": 71, "type": "PipeWire:Interface:Node", "info": { "props": {
          "media.class": "Audio/Sink",
          "node.name": "alsa_output.speaker",
          "node.description": "Arrow Lake cAVS Speaker",
          "device.api": "alsa", "device.bus": "pci" },
        "params": { "Props": [ { "volume": 1.0, "mute": true,
                                 "channelVolumes": [0.274625, 0.274625] } ] } } },
      { "id": 95, "type": "PipeWire:Interface:Node", "info": { "props": {
          "media.class": "Audio/Sink",
          "node.name": "bluez_output.AA.1",
          "node.description": "EDIFIER W830NB",
          "device.api": "bluez5" },
        "params": { "Props": [ { "volume": 1.0, "mute": false,
                                 "channelVolumes": [0.421875, 0.421875] } ] } } },
      { "id": 64, "type": "PipeWire:Interface:Node", "info": { "props": {
          "media.class": "Audio/Source",
          "node.name": "alsa_input.builtin",
          "node.description": "Arrow Lake cAVS Digital Microphone",
          "device.api": "alsa", "device.bus": "pci" },
        "params": { "Props": [ { "volume": 1.0, "mute": false,
                                 "channelVolumes": [1.0, 1.0] } ] } } },
      { "id": 69, "type": "PipeWire:Interface:Node", "info": { "props": {
          "media.class": "Audio/Source",
          "node.name": "alsa_input.headset",
          "node.description": "Headset Microphone",
          "device.api": "alsa", "device.bus": "usb" },
        "params": { "Props": [ { "volume": 1.0, "mute": true,
                                 "channelVolumes": [0.512, 0.512] } ] } } }
    ]"#;
    serde_json::from_str(text).expect("the fixture is valid JSON")
}

fn snapshot() -> AudioSnapshot {
    LiveGraph::of(&document()).snapshot()
}

#[test]
fn a_routing_write_covers_both_the_preference_and_the_live_default() {
    // Measured against WirePlumber 0.5.14: writing only
    // `default.configured.audio.<kind>` leaves the live `default.audio.<kind>`
    // key — and every running stream's link — untouched, so a switch made
    // through the shipping path would move nothing while the panel's marker
    // (read from the live key) also never moved.
    //
    // The keys are constants, so this asserts the pairing rather than the
    // command: a future edit that collapses the four keys into one loses either
    // the persisted preference or the live move, and fails here.
    for (configured, live) in [
        (CONFIGURED_SINK_KEY, DEFAULT_SINK_KEY),
        (CONFIGURED_SOURCE_KEY, DEFAULT_SOURCE_KEY),
    ] {
        assert!(configured.starts_with("default.configured.audio."));
        assert!(live.starts_with("default.audio."));
        assert_ne!(configured, live);
    }
    // The two directions are distinct keys: a source write cannot move playback.
    assert_ne!(DEFAULT_SINK_KEY, DEFAULT_SOURCE_KEY);
    assert_ne!(CONFIGURED_SINK_KEY, CONFIGURED_SOURCE_KEY);
}

// ---- invariants ----------------------------------------------------------

#[test]
fn test_inv_audio_1_every_node_handle_is_the_servers_name() {
    // `[INV-AUDIO-1]`: the handle is the server's `node.name`, in both
    // directions. The inventories carry no node id at all, so no ordinal can
    // reach a `SystemAction`, a status field, or a CLI argument.
    let snapshot = snapshot();
    let names: Vec<&str> = snapshot
        .outputs
        .iter()
        .map(|output| output.name.as_str())
        .chain(snapshot.inputs.iter().map(|input| input.name.as_str()))
        .collect();
    for name in &names {
        assert!(
            name.parse::<u64>().is_err(),
            "{name:?} is a number, not a stable node name"
        );
    }
    // The fixture's node ids (61, 64, 69, 71, 95) must not appear anywhere.
    for id in ["61", "64", "69", "71", "95"] {
        assert!(!names.contains(&id), "node id {id} leaked as a handle");
    }
}

#[test]
fn test_inv_audio_2_each_direction_marks_its_default_from_the_metadata() {
    // `[INV-AUDIO-2]`: exactly one authority marks each default, and it is the
    // server's `default` metadata object for that direction — never a shared
    // marker, never a cached click.
    let snapshot = snapshot();
    assert_eq!(
        snapshot
            .outputs
            .iter()
            .filter(|output| output.is_default)
            .map(|output| output.name.as_str())
            .collect::<Vec<_>>(),
        ["bluez_output.AA.1"],
        "the sink marker comes from `default.audio.sink`"
    );
    assert_eq!(
        snapshot
            .inputs
            .iter()
            .filter(|input| input.is_default)
            .map(|input| input.name.as_str())
            .collect::<Vec<_>>(),
        ["alsa_input.headset"],
        "the source marker comes from `default.audio.source`"
    );
    // The two keys are distinct: moving one cannot mark the other.
    let mut objects = document();
    for object in &mut objects {
        if object.get("type").and_then(Value::as_str) == Some("PipeWire:Interface:Metadata") {
            object["metadata"] = json!([
              { "subject": 0, "key": "default.audio.sink",
                "value": { "name": "alsa_input.headset" }, "type": "Spa:String:JSON" }
            ]);
        }
    }
    let crossed = LiveGraph::of(&objects).snapshot();
    assert!(
        crossed.outputs.iter().all(|output| !output.is_default),
        "a source name in the sink key marks nothing"
    );
    assert!(crossed.inputs.iter().all(|input| !input.is_default));
}

#[test]
fn test_inv_audio_5_empty_inventory_means_no_sound_server() {
    // `[INV-AUDIO-5]`: an empty inventory is "no sound server answered", and it
    // is expressed as an absent level rather than a zeroed fader. A fader pinned
    // at zero would claim the machine has no hardware.
    let absent = AudioSnapshot::absent();
    assert!(absent.outputs.is_empty() && absent.inputs.is_empty());
    assert_eq!(absent.output_volume, None);
    assert_eq!(absent.input_volume, None);
}

#[test]
fn test_inv_audio_7_capture_is_observed_from_the_link_graph() {
    // `[INV-AUDIO-7]`: capture is a fact about the server's links, not about
    // what the desktop dispatched. The three cases below are the ones a
    // plausible implementation gets wrong, and each failure mode is silent:
    // a stream node is not a capture, a sink monitor is not a microphone, and a
    // source feeding a capture stream is.
    let mut objects = document();
    objects.push(json!({
        "id": 96, "type": "PipeWire:Interface:Node",
        "info": { "props": {
            "media.class": "Stream/Input/Audio", "node.name": "pw-record" } }
    }));
    assert!(
        !LiveGraph::of(&objects).snapshot().capturing,
        "an unlinked stream node is a hypothesis, not a capture"
    );
    let mut monitor = objects.clone();
    monitor.push(json!({
        "id": 108, "type": "PipeWire:Interface:Link",
        "info": { "output-node-id": 71, "input-node-id": 96, "state": "active" }
    }));
    assert!(
        !LiveGraph::of(&monitor).snapshot().capturing,
        "a sink monitor is screen recording with sound, not microphone use"
    );
    let mut microphone = objects;
    microphone.push(json!({
        "id": 107, "type": "PipeWire:Interface:Link",
        "info": { "output-node-id": 64, "input-node-id": 96, "state": "active" }
    }));
    assert!(
        LiveGraph::of(&microphone).snapshot().capturing,
        "a source feeding a capture stream is a live microphone"
    );
}

// ---- inventory -----------------------------------------------------------

#[test]
fn an_unnamed_node_is_not_addressable_but_is_still_a_capture_source() {
    // Two questions, two answers. A node with no `node.name` cannot be named by
    // an intent, so it must not appear in the picker; but if it is an
    // `Audio/Source` and something links to it, the microphone is live and the
    // indicator must say so. Conflating the two would let an unnamed source
    // record silently.
    let document: Vec<Value> = serde_json::from_str(
        r#"[
          { "id": 5, "type": "PipeWire:Interface:Node", "info": { "props": {
              "media.class": "Audio/Source", "node.description": "Anonymous" } } },
          { "id": 96, "type": "PipeWire:Interface:Node", "info": { "props": {
              "media.class": "Stream/Input/Audio", "node.name": "pw-record" } } },
          { "id": 107, "type": "PipeWire:Interface:Link",
            "info": { "output-node-id": 5, "input-node-id": 96, "state": "active" } }
        ]"#,
    )
    .expect("valid JSON");
    let snapshot = LiveGraph::of(&document).snapshot();
    assert!(
        snapshot.inputs.is_empty(),
        "an unnamed node has no handle an intent could carry"
    );
    assert!(
        snapshot.capturing,
        "but a client is still recording from it"
    );
}

#[test]
fn outputs_are_grouped_by_transport_with_the_server_default_marked() {
    let snapshot = snapshot();
    let names: Vec<&str> = snapshot
        .outputs
        .iter()
        .map(|output| output.name.as_str())
        .collect();
    // Speaker (0) ranks before Bluetooth (2), then HDMI (4), so the list does
    // not reshuffle when the server reorders its nodes.
    assert_eq!(
        names,
        [
            "alsa_output.speaker",
            "bluez_output.AA.1",
            "alsa_output.hdmi1"
        ],
        "outputs ordered by transport rank, then name"
    );
    assert_eq!(snapshot.outputs[0].kind, AudioOutputKind::Speaker);
    assert_eq!(snapshot.outputs[1].kind, AudioOutputKind::Bluetooth);
    assert_eq!(snapshot.outputs[2].kind, AudioOutputKind::Hdmi);
    assert_eq!(
        snapshot
            .outputs
            .iter()
            .filter(|output| output.is_default)
            .map(|output| output.name.as_str())
            .collect::<Vec<_>>(),
        ["bluez_output.AA.1"],
        "exactly the node the metadata object names"
    );
    assert_eq!(snapshot.outputs[1].description, "EDIFIER W830NB");
}

#[test]
fn inputs_are_the_capture_side_and_never_appear_as_outputs() {
    let snapshot = snapshot();
    let inputs: Vec<&str> = snapshot
        .inputs
        .iter()
        .map(|input| input.name.as_str())
        .collect();
    // BuiltIn (0) before Headset/USB (3).
    assert_eq!(inputs, ["alsa_input.builtin", "alsa_input.headset"]);
    assert!(
        !snapshot
            .outputs
            .iter()
            .any(|output| output.name.starts_with("alsa_input")),
        "a source is not an output"
    );
    assert!(
        !snapshot
            .inputs
            .iter()
            .any(|input| input.name.starts_with("alsa_output")),
        "a sink is not an input"
    );
    assert_eq!(
        snapshot
            .inputs
            .iter()
            .find(|input| input.is_default)
            .map(|input| input.name.as_str()),
        Some("alsa_input.headset"),
        "the default source is a separate metadata key"
    );
}

#[test]
fn the_default_marker_is_read_from_either_metadata_value_shape() {
    // The fixture carries the object form for the sink and the raw JSON text for
    // the source. Both are what different server versions emit; accepting only
    // one silently drops the marker and leaves the picker unmarked.
    let snapshot = snapshot();
    assert_eq!(
        snapshot
            .outputs
            .iter()
            .find(|output| output.is_default)
            .map(|output| output.name.as_str()),
        Some("bluez_output.AA.1")
    );
    assert_eq!(
        snapshot
            .inputs
            .iter()
            .find(|input| input.is_default)
            .map(|input| input.name.as_str()),
        Some("alsa_input.headset")
    );
}

#[test]
fn exactly_one_default_per_half_even_when_the_server_names_an_absent_node() {
    // A metadata key can outlive the node it names (a device left range between
    // the node removal and the metadata update). The inventory then simply has
    // no marker rather than a fabricated one, and never two.
    let objects = document();
    let live = LiveGraph::of(&objects);
    assert_eq!(
        live.default_name(DEFAULT_SINK_KEY).as_deref(),
        Some("bluez_output.AA.1")
    );

    let mut absent = document();
    for object in &mut absent {
        if object.get("type").and_then(Value::as_str) == Some("PipeWire:Interface:Metadata") {
            object["metadata"] = json!([
              { "subject": 0, "key": "default.audio.sink",
                "value": { "name": "gone.node" }, "type": "Spa:String:JSON" }
            ]);
        }
    }
    let live = LiveGraph::of(&absent);
    let snapshot = live.snapshot();
    assert!(
        snapshot.outputs.iter().all(|output| !output.is_default),
        "a name that resolves to nothing marks nothing"
    );
}

// ---- volume and mute -----------------------------------------------------

#[test]
fn volume_is_recovered_from_the_servers_cubed_channel_value() {
    // PipeWire stores cubed amplitude. The default sink here reports 0.421875
    // per channel; reading that back verbatim would show 42% and make the panel
    // disagree with `wpctl`. The cube root recovers the position the user set:
    // 0.421875^(1/3) = 0.75.
    let snapshot = snapshot();
    assert_eq!(snapshot.output_volume, Some(75));
    assert!(!snapshot.output_muted);
    // The default *input* is the USB headset, reported at 0.512 per channel —
    // 0.512^(1/3) = 0.8 — and muted.
    assert_eq!(snapshot.input_volume, Some(80));
    assert!(snapshot.input_muted);
    // The mute is read from the node the default names, not from any node: the
    // built-in speaker is muted in the same document and must not leak its
    // state onto the default.
    assert!(
        snapshot
            .outputs
            .iter()
            .any(|output| output.name == "alsa_output.speaker"),
        "the muted non-default sink is still in the inventory"
    );
}

#[test]
fn a_node_without_a_volume_parameter_reports_unknown_not_zero() {
    // Virtual, filter, and adapter nodes publish no `Props` at all. Reporting
    // `Some(0)` would draw a fader pinned to silence.
    let document: Vec<Value> = serde_json::from_str(
        r#"[{ "id": 7, "type": "PipeWire:Interface:Node", "info": { "props": {
              "media.class": "Audio/Source", "node.name": "filter.node" } } }]"#,
    )
    .expect("valid JSON");
    let snapshot = LiveGraph::of(&document).snapshot();
    assert_eq!(snapshot.input_volume, None);
    assert!(!snapshot.input_muted);
    assert_eq!(snapshot.inputs.len(), 1);
}

#[test]
fn an_unparseable_or_empty_dump_reads_as_no_sound_server() {
    assert!(as_document("not json".to_owned()).is_none());
    assert!(as_document("{}".to_owned()).is_none());
    assert_eq!(as_document("[]".to_owned()), Some(Vec::new()));
    let empty = LiveGraph::of(&[]).snapshot();
    assert_eq!(empty, AudioSnapshot::absent());
    assert!(empty.outputs.is_empty());
    assert!(empty.inputs.is_empty());
    assert!(!empty.capturing);
}

// ---- capture detection ---------------------------------------------------

#[test]
fn a_link_from_a_source_to_a_stream_is_a_live_capture() {
    let mut objects = document();
    objects.push(json!({
        "id": 96, "type": "PipeWire:Interface:Node",
        "info": { "props": {
            "media.class": "Stream/Input/Audio",
            "node.name": "pw-record" } }
    }));
    objects.push(json!({
        "id": 107, "type": "PipeWire:Interface:Link",
        "info": { "output-node-id": 64, "input-node-id": 96, "state": "active" }
    }));
    assert!(
        LiveGraph::of(&objects).snapshot().capturing,
        "a source feeding a capture stream is recording"
    );
}

#[test]
fn a_link_from_a_sink_monitor_is_not_a_microphone_capture() {
    // Recording the desktop's own playback links a *sink's* monitor to a
    // Stream/Input/Audio node. That is screen recording, not microphone use, and
    // an indicator that conflated them would accuse a user of recording audio
    // whenever they recorded a window with sound.
    let mut objects = document();
    objects.push(json!({
        "id": 96, "type": "PipeWire:Interface:Node",
        "info": { "props": {
            "media.class": "Stream/Input/Audio",
            "node.name": "pw-record" } }
    }));
    objects.push(json!({
        "id": 108, "type": "PipeWire:Interface:Link",
        "info": { "output-node-id": 71, "input-node-id": 96, "state": "active" }
    }));
    assert!(!LiveGraph::of(&objects).snapshot().capturing);
}

#[test]
fn a_capture_stream_with_no_link_is_not_recording() {
    // The stream node exists but the server has not linked it (a stream waiting
    // for a target, or one whose source just vanished). The link is the
    // observation; the node's mere existence is not.
    let mut objects = document();
    objects.push(json!({
        "id": 96, "type": "PipeWire:Interface:Node",
        "info": { "props": {
            "media.class": "Stream/Input/Audio", "node.name": "pw-record" } }
    }));
    assert!(!LiveGraph::of(&objects).snapshot().capturing);
}

// ---- classification ------------------------------------------------------

#[test]
fn output_transports_are_classified_from_the_servers_own_vocabulary() {
    assert_eq!(
        classify_output("bluez_output.X.1", "Headset", Some("bluez5"), None),
        AudioOutputKind::Bluetooth
    );
    assert_eq!(
        classify_output(
            "alsa_output.pci-hdmi",
            "Arrow Lake cAVS HDMI / DisplayPort 2",
            Some("alsa"),
            Some("pci")
        ),
        AudioOutputKind::Hdmi
    );
    assert_eq!(
        classify_output(
            "alsa_output.pci-headphones",
            "Arrow Lake cAVS Headphones",
            Some("alsa"),
            Some("pci")
        ),
        AudioOutputKind::Wired
    );
    assert_eq!(
        classify_output(
            "alsa_output.usb-dac",
            "USB Audio DAC",
            Some("alsa"),
            Some("usb")
        ),
        AudioOutputKind::Usb
    );
    assert_eq!(
        classify_output(
            "alsa_output.pci-speaker",
            "Arrow Lake cAVS Speaker",
            Some("alsa"),
            Some("pci")
        ),
        AudioOutputKind::Speaker
    );
    // An API the desktop does not model is never guessed into a category a user
    // would act on.
    assert_eq!(
        classify_output("some.node", "Some Node", Some("libcamera"), None),
        AudioOutputKind::Other
    );
    assert_eq!(
        classify_output("some.node", "Some Node", None, None),
        AudioOutputKind::Other
    );
}

#[test]
fn input_transports_are_classified_from_the_servers_own_vocabulary() {
    assert_eq!(
        classify_input("bluez_input.AA.1", "WH-1000XM5", Some("bluez5"), None),
        AudioInputKind::Bluetooth
    );
    assert_eq!(
        classify_input(
            "alsa_input.pci-mic",
            "Arrow Lake cAVS Digital Microphone",
            Some("alsa"),
            Some("pci")
        ),
        AudioInputKind::BuiltIn
    );
    assert_eq!(
        classify_input(
            "alsa_input.usb-headset",
            "Headset Microphone",
            Some("alsa"),
            Some("usb")
        ),
        AudioInputKind::Usb,
        "the bus outranks the label: a USB headset is a USB device"
    );
    assert_eq!(
        classify_input(
            "alsa_input.pci-headset",
            "Front Headset Microphone",
            Some("alsa"),
            Some("pci")
        ),
        AudioInputKind::Headset
    );
    // A webcam's audio node is a microphone; V4L2 is the server's statement
    // about the device, not about what the user is choosing.
    assert_eq!(
        classify_input(
            "v4l2_input.usb-webcam",
            "Integrated Camera",
            Some("v4l2"),
            Some("usb")
        ),
        AudioInputKind::Usb
    );
    assert_eq!(
        classify_input("odd.node", "Odd Node", Some("jack"), None),
        AudioInputKind::Other
    );
}

// ---- mock contract -------------------------------------------------------

#[test]
fn the_mock_refuses_an_unknown_name_like_the_server_does() {
    // `pw-metadata` exits 0 for an unresolvable name and WirePlumber then
    // *persists* it, so the shipping path refuses it before the write
    // (`[INV-AUDIO-2]`). The mock must not be more permissive than that, or a
    // test would pass on behaviour the machine does not have.
    let mock = MockAudioBackend::new();
    assert!(mock.set_output("mock_output_headphones").is_ok());
    assert!(mock.set_output("mock_output_nonexistent").is_err());
    assert!(mock.set_input("mock_input_headset").is_ok());
    assert!(mock.set_input("mock_input_nonexistent").is_err());
}

#[test]
fn the_mock_keeps_exactly_one_default_per_half() {
    let mock = MockAudioBackend::new();
    for half in [
        |snapshot: &AudioSnapshot| {
            snapshot
                .outputs
                .iter()
                .filter(|node| node.is_default)
                .map(|node| node.name.clone())
                .collect::<Vec<_>>()
        },
        |snapshot: &AudioSnapshot| {
            snapshot
                .inputs
                .iter()
                .filter(|node| node.is_default)
                .map(|node| node.name.clone())
                .collect::<Vec<_>>()
        },
    ] {
        let before = half(&mock.snapshot());
        assert_eq!(before.len(), 1, "the mock starts with exactly one default");
    }
    let _ = mock.set_output("mock_output_headphones");
    let _ = mock.set_input("mock_input_headset");
    let snapshot = mock.snapshot();
    assert_eq!(
        snapshot
            .outputs
            .iter()
            .find(|output| output.is_default)
            .map(|output| output.name.as_str()),
        Some("mock_output_headphones")
    );
    assert_eq!(
        snapshot
            .inputs
            .iter()
            .find(|input| input.is_default)
            .map(|input| input.name.as_str()),
        Some("mock_input_headset")
    );
}

#[test]
fn setting_a_level_clears_the_mute_but_stepping_it_does_not() {
    // Two behaviours, one reason each, and the pair is the point.
    //
    // `wpctl set-volume` on a muted node leaves it `[MUTED]` (measured against
    // WirePlumber 0.5.14, sink and source alike), so the level write issues its
    // own `set-mute 0`. A desktop that assumed the server cleared it would raise
    // the level, claim the mute was gone, and leave the user dragging a fader
    // that produces no sound.
    //
    // A *step* deliberately does not: it is ambient input that can be pressed by
    // accident on a machine someone muted on purpose. The level moves, the glyph
    // stays, and the user can see why nothing is audible.
    let mock = MockAudioBackend::new();
    let _ = mock.toggle_output_mute();
    assert!(mock.snapshot().output_muted);
    let _ = mock.set_output_volume(40);
    let snapshot = mock.snapshot();
    assert!(!snapshot.output_muted, "a level write ends the mute");
    assert_eq!(snapshot.output_volume, Some(40));

    let _ = mock.toggle_output_mute();
    let _ = mock.step_output_volume(5);
    let snapshot = mock.snapshot();
    assert_eq!(snapshot.output_volume, Some(45));
    assert!(
        snapshot.output_muted,
        "a step leaves a deliberate mute alone"
    );
}

#[test]
fn the_microphone_level_write_clears_its_mute_the_same_way() {
    // The capture half obeys the same contract as the playback half, for the
    // same reason: a muted microphone whose fader moves looks broken, and the
    // server will not clear the mute by itself (ADR-0216).
    let mock = MockAudioBackend::new();
    let _ = mock.toggle_input_mute();
    assert!(mock.snapshot().input_muted);
    let _ = mock.set_input_volume(15);
    let snapshot = mock.snapshot();
    assert!(!snapshot.input_muted);
    assert_eq!(snapshot.input_volume, Some(15));
}

#[test]
fn stepping_the_mock_volume_clamps_to_the_ends() {
    let mock = MockAudioBackend::new();
    let _ = mock.set_output_volume(95);
    let _ = mock.step_output_volume(20);
    assert_eq!(mock.snapshot().output_volume, Some(100));
    let _ = mock.step_output_volume(-120);
    assert_eq!(mock.snapshot().output_volume, Some(0));
}
