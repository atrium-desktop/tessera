//! End to end: `tessera-cast cast` to the scripted `tessera-cast sink`, with the
//! P2P radio provided by a fake NetworkManager on a private bus.
//!
//! Both sides are the real production binary as separate processes; the
//! RTSP exchange, the RTP/MPEG-TS media, and the D-Bus radio calls all cross
//! real sockets. Only the system daemon and the television are stand-ins, so
//! this verifies a NetworkManager-host cast in any environment without a TV.

use std::net::TcpListener;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use crate::common::binary;
use crate::common::fake_networkmanager::{Activation, FakeNetworkManager, TV_NAME};
use crate::common::poll::wait_until;

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .and_then(|listener| listener.local_addr())
        .map(|address| address.port())
        .expect("a free port")
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn a_cast_over_networkmanager_streams_to_the_sink_and_leaves_the_group() {
    let Some(fake) = FakeNetworkManager::start(Activation::Activate) else {
        eprintln!("dbus-daemon is not installed; skipping");
        return;
    };
    let caster = binary("tessera-cast");
    let port = free_port();

    let source = Command::new(&caster)
        .args([
            "cast",
            "--peer",
            "创维",
            "--settle",
            "0",
            "--bind",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "--source",
            "pattern",
            "--video-formats",
            "00 00 02 10",
            "--max-frames",
            "30",
            "--timeout",
            "20",
        ])
        .env("DBUS_SYSTEM_BUS_ADDRESS", fake.address())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn the caster");

    // The sink can only reach the source once the P2P group exists.
    assert!(
        wait_until(Duration::from_secs(20), || fake.group_is_up()),
        "the caster never formed the group: {:?}",
        fake.calls()
    );

    let sink = Command::new(&caster)
        .args([
            "sink",
            "--connect",
            &format!("127.0.0.1:{port}"),
            "--play",
            "--rtp-port",
            "19000",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn the sink");

    let source = source.wait_with_output().expect("caster output");
    let sink = sink.wait_with_output().expect("sink output");
    let source_text = text(&source);
    let sink_text = text(&sink);

    assert!(source.status.success(), "caster:\n{source_text}");
    assert!(sink.status.success(), "sink:\n{sink_text}");
    assert!(
        source_text.contains("radio: NetworkManager"),
        "{source_text}"
    );
    assert!(
        source_text.contains(&format!("P2P group with {TV_NAME} is up")),
        "{source_text}"
    );
    assert!(
        source_text.contains("sink accepted the handshake"),
        "{source_text}"
    );
    assert!(
        source_text.contains("stream completed: 30 frames"),
        "{source_text}"
    );
    assert!(
        sink_text.contains("sink completed the negotiation"),
        "{sink_text}"
    );
    assert!(sink_text.contains("media received:"), "{sink_text}");

    let calls = fake.calls();
    assert!(
        calls
            .iter()
            .any(|call| call.starts_with("DeactivateConnection")),
        "the caster must leave the group it formed: {calls:?}"
    );
    assert!(!fake.group_is_up());
}

#[test]
fn a_cast_without_permission_fails_fast_and_names_the_cause() {
    let Some(fake) = FakeNetworkManager::start(Activation::Deny) else {
        eprintln!("dbus-daemon is not installed; skipping");
        return;
    };
    let output = Command::new(binary("tessera-cast"))
        .args([
            "cast",
            "--peer",
            "创维",
            "--settle",
            "0",
            "--bind",
            "127.0.0.1",
            "--port",
            &free_port().to_string(),
            "--source",
            "pattern",
            "--video-formats",
            "00 00 02 10",
            "--timeout",
            "20",
        ])
        .env("DBUS_SYSTEM_BUS_ADDRESS", fake.address())
        .output()
        .expect("run the caster");
    let output_text = text(&output);
    assert_eq!(output.status.code(), Some(1), "{output_text}");
    assert!(
        output_text.contains("not authorized to form the P2P group"),
        "{output_text}"
    );
    let attempts = fake
        .calls()
        .iter()
        .filter(|call| call.starts_with("AddAndActivateConnection2"))
        .count();
    assert_eq!(attempts, 1, "a missing privilege is not retried");
}

#[test]
fn discover_lists_only_the_tv_as_castable() {
    let Some(fake) = FakeNetworkManager::start(Activation::Activate) else {
        eprintln!("dbus-daemon is not installed; skipping");
        return;
    };
    let output = Command::new(binary("tessera-cast"))
        .args(["discover", "--settle", "0"])
        .env("DBUS_SYSTEM_BUS_ADDRESS", fake.address())
        .output()
        .expect("run discover");
    let output_text = text(&output);
    assert_eq!(output.status.code(), Some(0), "{output_text}");
    assert!(
        output_text.contains(&format!("[castable] {TV_NAME}")),
        "{output_text}"
    );
    assert!(
        output_text.contains("[skipped]  DIRECT-e8-HP M233 LaserJet"),
        "{output_text}"
    );
}

#[test]
fn sigterm_after_group_formation_leaves_the_group() {
    let Some(fake) = FakeNetworkManager::start(Activation::Activate) else {
        eprintln!("dbus-daemon is not installed; skipping");
        return;
    };
    let caster = binary("tessera-cast");
    let port = free_port();

    let mut child = Command::new(&caster)
        .args([
            "cast",
            "--peer",
            "创维",
            "--settle",
            "0",
            "--bind",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "--source",
            "pattern",
            "--video-formats",
            "00 00 02 10",
            "--timeout",
            "30",
        ])
        .env("DBUS_SYSTEM_BUS_ADDRESS", fake.address())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn the caster");

    assert!(
        wait_until(Duration::from_secs(20), || fake.group_is_up()),
        "the caster never formed the group: {:?}",
        fake.calls()
    );

    // SIGTERM must unwind through the session's group guard, not terminate
    // between destructors.
    unsafe {
        libc::kill(child.id() as i32, libc::SIGTERM);
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if child.try_wait().expect("wait for the caster").is_some() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "caster did not stop after SIGTERM: {:?}",
            fake.calls()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let output = child.wait_with_output().expect("caster output");
    let output_text = text(&output);
    assert!(output.status.success(), "caster:\n{output_text}");
    assert!(
        output_text.contains("P2P group with"),
        "the group was never formed:\n{output_text}"
    );
    assert!(
        output_text.contains("session stopped"),
        "the caster did not report a graceful stop:\n{output_text}"
    );
    let calls = fake.calls();
    assert!(
        calls
            .iter()
            .any(|call| call.starts_with("DeactivateConnection")),
        "SIGTERM must leave the group: {calls:?}"
    );
    assert!(!fake.group_is_up(), "the group is still active: {calls:?}");
}
