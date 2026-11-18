//! Casting over NetworkManager, against a fake NetworkManager on a private bus.
//!
//! These drive the real `tessera_cast` radio selection, discovery, group
//! formation, and session code through D-Bus — only the daemon on the other
//! side is fake. `DBUS_SYSTEM_BUS_ADDRESS` points the caster at the private
//! bus, so nothing here can reach the host's real NetworkManager or radio.

use std::net::TcpStream;
use std::time::Duration;

use tessera_cast::discover;
use tessera_cast::radio::networkmanager::NmRadio;
use tessera_cast::radio::{self, DisplayCapabilities, P2pRadio};
use tessera_cast::session::{self, MediaSource, Radio, SessionEvent, SessionOptions};
use tessera_cast::sink::{self, SinkScript};
use tessera_cast::wfd::transport::RtspTransport;
use tessera_dbus::Value;

use crate::common::env::EnvGuard;
use crate::common::fake_networkmanager::{
    Activation, FakeNetworkManager, P2P_DEVICE, TV_ADDRESS, TV_NAME, TV_PEER,
};
use crate::common::poll::wait_until;

/// Start the fake and point the system bus at it, or skip.
fn fake(activation: Activation) -> Option<(FakeNetworkManager, EnvGuard)> {
    let Some(fake) = FakeNetworkManager::start(activation) else {
        eprintln!("dbus-daemon is not installed; skipping");
        return None;
    };
    let guard = EnvGuard::set("DBUS_SYSTEM_BUS_ADDRESS", fake.address());
    Some((fake, guard))
}

fn string(value: Option<Value>) -> Option<String> {
    value.and_then(|value| value.as_str().map(str::to_string))
}

#[test]
fn networkmanager_is_selected_when_iwd_is_not_running() {
    let Some((_fake, _env)) = fake(Activation::Activate) else {
        return;
    };
    let radio = radio::open_system().expect("a P2P authority");
    assert_eq!(radio.authority(), "NetworkManager");
}

#[test]
fn discovery_lists_the_tv_as_castable_and_skips_the_printers() {
    let Some((fake, _env)) = fake(Activation::Activate) else {
        return;
    };
    let radio = radio::open_system().expect("radio");
    let report = discover::discover(radio.as_ref(), 7236, Duration::ZERO).expect("discover");

    let castable: Vec<_> = report.castable().collect();
    assert_eq!(castable.len(), 1, "{}", report.render());
    assert_eq!(castable[0].name.as_deref(), Some(TV_NAME));
    assert_eq!(castable[0].device_address.as_deref(), Some(TV_ADDRESS));
    assert_eq!(
        castable[0].display.and_then(|display| display.port),
        Some(7236)
    );
    for printer in report.peers.iter().filter(|peer| peer.path != TV_PEER) {
        assert_eq!(
            printer.cast_blocker(),
            Some("advertises no Wi-Fi Display capability")
        );
    }
    assert!(
        fake.calls()
            .contains(&"StartFind timeout=Some(30)".to_owned())
    );
}

#[test]
fn a_group_is_a_user_owned_volatile_profile_advertising_a_source() {
    let Some((fake, _env)) = fake(Activation::Activate) else {
        return;
    };
    let radio = NmRadio::open().expect("radio");
    radio
        .register_source(DisplayCapabilities::source_only(7236))
        .expect("register");
    radio.form_group(TV_PEER).expect("group");

    assert!(fake.group_is_up());
    assert!(
        fake.calls()
            .contains(&format!("AddAndActivateConnection2 {P2P_DEVICE} {TV_PEER}"))
    );
    assert_eq!(
        string(fake.setting("connection", "type")).as_deref(),
        Some("wifi-p2p")
    );
    assert_eq!(
        string(fake.setting("wifi-p2p", "peer")).as_deref(),
        Some(TV_ADDRESS)
    );
    let ies = fake
        .setting("wifi-p2p", "wfd-ies")
        .and_then(|value| <Vec<u8> as tessera_dbus::FromValue>::from_value(value).ok());
    assert_eq!(
        ies,
        Some(vec![0x00, 0x00, 0x06, 0x00, 0x10, 0x1c, 0x44, 0x00, 0x32]),
        "device information: role source, session available, port 7236"
    );
    let permissions = fake
        .setting("connection", "permissions")
        .and_then(|value| <Vec<String> as tessera_dbus::FromValue>::from_value(value).ok())
        .unwrap_or_default();
    assert_eq!(permissions.len(), 1, "{permissions:?}");
    assert!(
        permissions[0].starts_with("user:"),
        "a user-owned profile needs only settings.modify.own: {permissions:?}"
    );
    let persist = fake
        .state
        .lock()
        .unwrap()
        .options
        .as_ref()
        .and_then(|options| options.get("persist"))
        .cloned();
    assert_eq!(string(persist).as_deref(), Some("volatile"));

    let tv = radio
        .peers()
        .expect("peers")
        .into_iter()
        .find(|peer| peer.path == TV_PEER)
        .expect("tv");
    assert!(tv.connected);
    assert_eq!(tv.connected_interface.as_deref(), Some("p2p-wlan0-0"));

    radio.disconnect_peer(TV_PEER).expect("disconnect");
    assert!(!fake.group_is_up());
    assert!(
        fake.calls()
            .iter()
            .any(|call| call.starts_with("DeactivateConnection"))
    );
}

#[test]
fn dropping_the_radio_tears_down_its_group_and_stops_the_find() {
    let Some((fake, _env)) = fake(Activation::Activate) else {
        return;
    };
    {
        let radio = NmRadio::open().expect("radio");
        radio.request_discovery().expect("find");
        radio.form_group(TV_PEER).expect("group");
    }
    let calls = fake.calls();
    assert!(
        calls
            .iter()
            .any(|call| call.starts_with("DeactivateConnection")),
        "{calls:?}"
    );
    assert!(calls.contains(&"StopFind".to_owned()), "{calls:?}");
    assert!(!fake.group_is_up());
}

#[test]
fn a_failed_activation_is_reported_not_waited_out() {
    let Some((_fake, _env)) = fake(Activation::Fail) else {
        return;
    };
    let radio = NmRadio::open().expect("radio");
    let started = std::time::Instant::now();
    let error = radio.form_group(TV_PEER).expect_err("activation fails");
    assert!(
        error.to_string().contains("could not form the P2P group"),
        "{error}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "a failed state must end the wait, not the 45 s deadline"
    );
}

#[test]
fn a_refused_profile_is_an_authorization_failure() {
    let Some((_fake, _env)) = fake(Activation::Deny) else {
        return;
    };
    let radio = NmRadio::open().expect("radio");
    let error = radio.form_group(TV_PEER).expect_err("denied");
    assert!(
        matches!(error, tessera_cast::iwd::IwdError::NotAuthorized(_)),
        "{error:?}"
    );
}

fn session_options() -> SessionOptions {
    SessionOptions {
        bind: "127.0.0.1".to_owned(),
        control_port: 0,
        peer: Some("创维".to_owned()),
        radio: Radio::System,
        settle: Duration::ZERO,
        video_formats: "00 00 02 10".to_owned(),
        rtp_port: 19_000,
        timeout: Duration::from_secs(10),
        fps: 30,
        source: MediaSource::Pattern,
        duration: None,
        max_frames: None,
        // The media path is covered by the end-to-end suite; fixed RTP ports
        // would collide with it when the suites run concurrently.
        stream_media: false,
        ts_file: None,
        ts_seconds: 30,
        opening: tessera_cast::cast::Opening::Sink,
        audio: false,
        audio_source: tessera_cast::media::AudioSourceKind::System,
        audio_bitrate_kbps: 128,
        stop: None,
        group_attempts: 1,
        group_retry_delay: Duration::ZERO,
    }
}

#[test]
fn a_session_forms_the_group_negotiates_and_leaves_the_group() {
    let Some((fake, _env)) = fake(Activation::Activate) else {
        return;
    };
    let fake = std::sync::Arc::new(fake);
    let mut events = Vec::new();
    let mut sink_thread = None;
    let result = session::run(&session_options(), &mut |event| {
        if let SessionEvent::Listening(address) = &event {
            let address = *address;
            let fake = std::sync::Arc::clone(&fake);
            // A real sink can only connect once the P2P group exists.
            sink_thread = Some(std::thread::spawn(move || {
                assert!(wait_until(Duration::from_secs(10), || fake.group_is_up()));
                let stream = TcpStream::connect(address).expect("connect");
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .expect("timeout");
                let script = SinkScript {
                    expect_setup_play: true,
                    ..SinkScript::default()
                };
                sink::run_client(RtspTransport::new(stream), &script).expect("negotiation")
            }));
        }
        events.push(event);
    });
    let report = sink_thread.expect("listening").join().expect("sink");

    assert_eq!(result, Ok(()));
    assert!(report.cseq_matched);
    assert!(events.contains(&SessionEvent::RadioAuthority("NetworkManager".to_owned())));
    assert!(events.contains(&SessionEvent::GroupFormed(TV_NAME.to_owned())));
    assert!(events.contains(&SessionEvent::Streaming));
    assert!(!fake.group_is_up(), "the session left the group it formed");
}

#[test]
#[ignore = "requires a running Wi-Fi P2P authority (NetworkManager or iwd)"]
fn live_p2p_discovery_reaches_the_bus() {
    let radio = radio::open_system().expect("a live P2P authority");
    let report = discover::discover(radio.as_ref(), 7236, Duration::from_secs(2))
        .expect("live P2P discovery");
    eprintln!(
        "live P2P authority={} peers={} castable={}",
        radio.authority(),
        report.peers.len(),
        report.castable().count()
    );
}
