//! Session-level tests: a scripted P2P authority and the scripted sink over
//! loopback drive [`run_with_radio`] end to end, so group formation, retries,
//! and teardown are covered without a radio or a television.

use std::net::TcpStream;
use std::sync::Mutex;
use std::thread::JoinHandle;

use super::*;
use crate::iwd::IwdError;
use crate::radio::{DiscoveredPeer, DisplayCapabilities};
use crate::sink::{self, SinkScript};

/// A P2P authority that records every call and answers from a script.
struct ScriptedRadio {
    calls: Mutex<Vec<String>>,
    peers: Vec<DiscoveredPeer>,
    /// Answers to successive `form_group` calls; empty means `Ok`.
    form_results: Mutex<Vec<Result<(), IwdError>>>,
}

impl ScriptedRadio {
    fn new(peers: Vec<DiscoveredPeer>) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            peers,
            form_results: Mutex::new(Vec::new()),
        }
    }

    fn with_form_results(self, results: Vec<Result<(), IwdError>>) -> Self {
        *self.form_results.lock().unwrap() = results;
        self
    }

    fn record(&self, call: impl Into<String>) {
        self.calls.lock().unwrap().push(call.into());
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    fn count(&self, prefix: &str) -> usize {
        self.calls()
            .iter()
            .filter(|call| call.starts_with(prefix))
            .count()
    }
}

impl P2pRadio for ScriptedRadio {
    fn enable_p2p(&self) -> Result<(), IwdError> {
        self.record("enable");
        Ok(())
    }
    fn register_source(&self, _advertisement: DisplayCapabilities) -> Result<(), IwdError> {
        self.record("register");
        Ok(())
    }
    fn unregister_source(&self) -> Result<(), IwdError> {
        self.record("unregister");
        Ok(())
    }
    fn request_discovery(&self) -> Result<(), IwdError> {
        self.record("discover");
        Ok(())
    }
    fn peers(&self) -> Result<Vec<DiscoveredPeer>, IwdError> {
        self.record("peers");
        Ok(self.peers.clone())
    }
    fn form_group(&self, peer_path: &str) -> Result<(), IwdError> {
        self.record(format!("form {peer_path}"));
        let mut results = self.form_results.lock().unwrap();
        if results.is_empty() {
            Ok(())
        } else {
            results.remove(0)
        }
    }
    fn disconnect_peer(&self, peer_path: &str) -> Result<(), IwdError> {
        self.record(format!("disconnect {peer_path}"));
        Ok(())
    }
}

fn tv(connected: bool) -> DiscoveredPeer {
    DiscoveredPeer {
        path: "/peer/tv".to_owned(),
        name: Some("创维电视A3F_99E".to_owned()),
        device_address: Some("9E:1F:E6:7C:8B:CE".to_owned()),
        display: Some(DisplayCapabilities {
            source: false,
            sink: true,
            port: Some(7236),
            has_audio: false,
            has_uibc: false,
            has_content_protection: false,
        }),
        connected,
        connected_interface: connected.then(|| "p2p-wlan0-0".to_owned()),
        connected_ip: None,
    }
}

fn options(timeout: Duration) -> SessionOptions {
    SessionOptions {
        bind: "127.0.0.1".to_owned(),
        control_port: 0,
        peer: Some("创维".to_owned()),
        radio: Radio::System,
        settle: Duration::ZERO,
        video_formats: "00 00 02 10".to_owned(),
        rtp_port: 19_000,
        timeout,
        fps: 30,
        source: MediaSource::Pattern,
        duration: None,
        max_frames: None,
        // Handshake only: these tests are about the radio lifecycle, and the
        // fixed media ports would collide between concurrent tests.
        stream_media: false,
        ts_file: None,
        ts_seconds: 30,
        opening: Opening::Sink,
        audio: false,
        audio_source: AudioSourceKind::System,
        audio_bitrate_kbps: 128,
        stop: None,
        group_attempts: 3,
        group_retry_delay: Duration::ZERO,
    }
}

/// How the scripted sink behaves once the source is listening.
#[derive(Clone, Copy)]
enum Sink {
    /// Run the full sink-driven negotiation through PLAY.
    Negotiate,
    /// Connect, then hang up without a word.
    HangUp,
    /// Never connect.
    Absent,
}

/// Run a session against `radio`, starting the scripted sink when the source
/// reports its listening address.
fn run_session(
    options: &SessionOptions,
    radio: &ScriptedRadio,
    sink: Sink,
) -> (Result<(), String>, Vec<SessionEvent>) {
    let mut events = Vec::new();
    let mut sink_thread: Option<JoinHandle<()>> = None;
    let result = run_with_radio(options, Some(radio), &mut |event| {
        if let SessionEvent::Listening(address) = &event {
            let address = *address;
            sink_thread = match sink {
                Sink::Absent => None,
                Sink::HangUp => Some(std::thread::spawn(move || {
                    drop(TcpStream::connect(address).expect("connect"));
                })),
                Sink::Negotiate => Some(std::thread::spawn(move || {
                    let stream = TcpStream::connect(address).expect("connect");
                    stream
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .expect("timeout");
                    let script = SinkScript {
                        expect_setup_play: true,
                        ..SinkScript::default()
                    };
                    sink::run_client(RtspTransport::new(stream), &script)
                        .expect("the sink completes the negotiation");
                })),
            };
        }
        events.push(event);
    });
    if let Some(thread) = sink_thread {
        thread.join().expect("sink thread");
    }
    (result, events)
}

#[test]
fn a_group_formed_for_the_session_is_torn_down_after_it() {
    let radio = ScriptedRadio::new(vec![tv(false)]);
    let (result, events) = run_session(&options(Duration::from_secs(5)), &radio, Sink::Negotiate);
    assert_eq!(result, Ok(()));
    assert_eq!(
        radio.calls(),
        vec![
            "enable",
            "register",
            "discover",
            "peers",
            "form /peer/tv",
            "disconnect /peer/tv"
        ]
    );
    assert!(events.contains(&SessionEvent::RadioAuthority("scripted".to_owned())));
    assert!(events.contains(&SessionEvent::GroupFormed("创维电视A3F_99E".to_owned())));
    assert!(events.contains(&SessionEvent::Streaming));
    assert!(matches!(events.last(), Some(SessionEvent::Stopped(_))));
}

#[test]
fn a_failed_handshake_still_leaves_the_group() {
    let radio = ScriptedRadio::new(vec![tv(false)]);
    let (result, events) = run_session(&options(Duration::from_secs(5)), &radio, Sink::HangUp);
    // An immediate hang-up surfaces as a failed or refused negotiation,
    // depending on where the source notices it; either way it is a failure.
    let error = result.unwrap_err();
    assert!(
        error.contains("handshake failed") || error.contains("did not accept streaming"),
        "{error}"
    );
    assert!(matches!(events.last(), Some(SessionEvent::Failed(_))));
    assert_eq!(radio.count("disconnect /peer/tv"), 1);
}

#[test]
fn a_sink_that_never_connects_still_leaves_the_group() {
    let radio = ScriptedRadio::new(vec![tv(false)]);
    let (result, _events) = run_session(&options(Duration::from_millis(300)), &radio, Sink::Absent);
    assert!(result.is_err());
    assert_eq!(radio.count("disconnect /peer/tv"), 1);
}

#[test]
fn a_cancelled_wait_still_leaves_the_group() {
    let radio = ScriptedRadio::new(vec![tv(false)]);
    let mut options = options(Duration::from_secs(5));
    let stop = Arc::new(AtomicBool::new(false));
    options.stop = Some(stop.clone());
    // Let the scripted group form, then cancel while waiting for the sink.
    let canceller = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(200));
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
    });
    let (result, events) = run_session(&options, &radio, Sink::Absent);
    canceller.join().expect("canceller");
    assert_eq!(result, Ok(()));
    assert!(matches!(events.last(), Some(SessionEvent::Stopped(_))));
    assert_eq!(radio.count("disconnect /peer/tv"), 1);
}

#[test]
fn a_precancelled_session_does_not_form_a_group() {
    let radio = ScriptedRadio::new(vec![tv(false)]);
    let mut options = options(Duration::from_secs(5));
    options.stop = Some(Arc::new(AtomicBool::new(true)));
    let (result, events) = run_session(&options, &radio, Sink::Absent);
    assert_eq!(result, Ok(()));
    assert!(matches!(
        events.last(),
        Some(SessionEvent::Stopped(reason)) if reason.contains("before forming")
    ));
    assert_eq!(radio.count("disconnect /peer/tv"), 0);
}

#[test]
fn a_transient_group_failure_is_retried() {
    let radio = ScriptedRadio::new(vec![tv(false)])
        .with_form_results(vec![Err(IwdError::Protocol("WPS timeout".to_owned()))]);
    let (result, _events) = run_session(&options(Duration::from_secs(5)), &radio, Sink::Negotiate);
    assert_eq!(result, Ok(()));
    assert_eq!(radio.count("form "), 2);
    assert_eq!(radio.count("disconnect "), 1);
}

#[test]
fn exhausted_group_attempts_fail_with_the_operator_action() {
    let failure = || Err(IwdError::Protocol("WPS timeout".to_owned()));
    let radio = ScriptedRadio::new(vec![tv(false)]).with_form_results(vec![
        failure(),
        failure(),
        failure(),
    ]);
    let (result, events) = run_session(&options(Duration::from_secs(5)), &radio, Sink::Absent);
    let error = result.unwrap_err();
    assert!(error.contains("after 3 attempts"), "{error}");
    assert!(error.contains("Screen Mirroring"), "{error}");
    assert!(matches!(events.last(), Some(SessionEvent::Failed(_))));
    assert_eq!(radio.count("form "), 3);
    assert_eq!(
        radio.count("disconnect "),
        0,
        "a group that never formed has nothing to tear down"
    );
}

#[test]
fn a_missing_privilege_is_not_retried() {
    let radio = ScriptedRadio::new(vec![tv(false)]).with_form_results(vec![Err(
        IwdError::NotAuthorized("Insufficient privileges".to_owned()),
    )]);
    let (result, _events) = run_session(&options(Duration::from_secs(5)), &radio, Sink::Absent);
    let error = result.unwrap_err();
    assert!(error.contains("not authorized"), "{error}");
    assert!(
        !error.contains("Screen Mirroring"),
        "toggling the TV cannot fix a privilege"
    );
    assert_eq!(radio.count("form "), 1);
}

#[test]
fn a_group_this_session_did_not_form_is_left_alone() {
    let radio = ScriptedRadio::new(vec![tv(true)]);
    let (result, events) = run_session(&options(Duration::from_secs(5)), &radio, Sink::Negotiate);
    assert_eq!(result, Ok(()));
    assert_eq!(radio.count("form "), 0);
    assert_eq!(radio.count("disconnect "), 0);
    assert!(events.contains(&SessionEvent::GroupFormed("创维电视A3F_99E".to_owned())));
}

#[test]
fn an_unknown_peer_is_refused_without_forming_anything() {
    let radio = ScriptedRadio::new(vec![tv(false)]);
    let mut options = options(Duration::from_secs(1));
    options.peer = Some("Bravia".to_owned());
    let (result, _events) = run_session(&options, &radio, Sink::Absent);
    assert_eq!(result, Err("no peer matched 'Bravia'".to_owned()));
    assert_eq!(radio.count("form "), 0);
}

#[test]
fn a_peer_without_a_radio_is_refused() {
    let mut events = Vec::new();
    let result = run_with_radio(&options(Duration::from_secs(1)), None, &mut |event| {
        events.push(event)
    });
    assert!(result.unwrap_err().contains("--peer needs the P2P radio"));
    assert!(events.is_empty(), "nothing is bound before the refusal");
}
