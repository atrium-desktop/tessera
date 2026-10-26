//! The observing layer: the only place that touches the OS, a bus, or a
//! subprocess. It produces [`Verdict`]s and never interprets them; the meaning
//! lives in [`crate::probe`].
//!
//! Every observation is **read-only**. The probe must be safe to run on a live
//! session at any time, so nothing here writes a property, enables an adapter,
//! or starts a scan. `net.connman.iwd.p2p.Device.Enabled` is read, never set —
//! enabling P2P is [`crate::discover`]'s action, not a diagnostic side effect.

use crate::iwd::{self, IwdError, P2P_DEVICE, P2P_SERVICE_MANAGER};
use crate::probe::{CastReport, P2pDeviceFacts, PipelineReport, RadioReport, Verdict};

/// How a shared-layer failure becomes a verdict.
///
/// The split is the whole point of the three-state vocabulary: "iwd is not
/// running", "this adapter has no P2P device", and "the interface does not exist"
/// are conclusive absences; a refused bus connection, a missing privilege, or an
/// unexpected answer are not, and must stay unknown ([`crate::probe`],
/// `[INV-ARCH-83]`).
fn verdict_from_error<T>(error: IwdError) -> Verdict<T> {
    match error {
        IwdError::ServiceUnavailable | IwdError::Unsupported(_) | IwdError::NoAdapter => {
            Verdict::Absent
        }
        // The probe sends no arguments, so an argument-shape refusal here means
        // the daemon answered something this build does not model: unknown, not
        // absent.
        IwdError::InvalidArguments(reason) => Verdict::Unverified(reason),
        IwdError::BusUnavailable(reason) => Verdict::Unverified(reason),
        IwdError::NotAuthorized(reason) => Verdict::Unverified(reason),
        IwdError::Protocol(reason) => Verdict::Unverified(reason),
    }
}

/// Observe both halves of the Miracast capability.
///
/// A failure to reach the session bus degrades only the pipeline verdicts that
/// need it; a failure to reach the system bus degrades only the radio verdicts.
/// Neither is promoted into a capability.
pub fn probe() -> CastReport {
    CastReport {
        radio: probe_radio(),
        pipeline: probe_pipeline(),
    }
}

fn probe_radio() -> RadioReport {
    let connection = match iwd::system_connection() {
        Ok(connection) => connection,
        Err(error) => {
            let reason = error.to_string();
            return RadioReport {
                wfd_registration: Verdict::Unverified(reason.clone()),
                p2p_device: Verdict::Unverified(reason),
            };
        }
    };
    RadioReport {
        wfd_registration: observe_wfd_registration(&connection),
        p2p_device: observe_p2p_device(&connection),
    }
}

/// Whether iwd offers `RegisterDisplayService`. Its presence is what makes iwd a
/// usable *radio* for a caster.
fn observe_wfd_registration(connection: &zbus::blocking::Connection) -> Verdict<()> {
    match iwd::has_interface(connection, iwd::IWD_ROOT, P2P_SERVICE_MANAGER) {
        Ok(true) => Verdict::Observed(()),
        Ok(false) => Verdict::Absent,
        Err(error) => verdict_from_error(error),
    }
}

/// Read the first adapter's P2P device facts. iwd exposes one `p2p.Device` per
/// adapter; a caster needs one it can use.
fn observe_p2p_device(connection: &zbus::blocking::Connection) -> Verdict<P2pDeviceFacts> {
    let adapter = match iwd::first_adapter(connection) {
        Ok(adapter) => adapter,
        Err(error) => return verdict_from_error(error),
    };
    if let Err(error) = iwd::has_interface(connection, &adapter, P2P_DEVICE) {
        return verdict_from_error(error);
    }
    let enabled = match iwd::bool_property(connection, &adapter, P2P_DEVICE, "Enabled") {
        Ok(value) => value,
        Err(error) => return verdict_from_error(error),
    };
    let available_connections =
        match iwd::u16_property(connection, &adapter, P2P_DEVICE, "AvailableConnections") {
            Ok(value) => value,
            Err(error) => return verdict_from_error(error),
        };
    Verdict::Observed(P2pDeviceFacts {
        enabled,
        available_connections,
    })
}

fn probe_pipeline() -> PipelineReport {
    PipelineReport {
        h264_encoder: observe_h264_encoder(),
        pipewire: observe_pipewire(),
        screencast_portal: observe_screencast_portal(),
    }
}

/// Ask `vainfo` for an H.264 encode entry point.
///
/// `vainfo` is the distribution-standard reporter for VA-API configurations and
/// reports entry points per driver; without it the verdict is unknown, never
/// assumed. Reading the driver's `.so` files would say only that a *driver* is
/// installed, not that an encode entry point exists, so it is deliberately not
/// used as a substitute.
fn observe_h264_encoder() -> Verdict<()> {
    let output = match std::process::Command::new("vainfo").output() {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Verdict::Unverified("vainfo is not installed".into());
        }
        Err(error) => return Verdict::Unverified(format!("vainfo failed to run: {error}")),
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let encoder = stdout
        .lines()
        .any(|line| line.contains("VAProfileH264") && line.contains("VAEntrypointEncSlice"));
    // A driver that initializes but exposes no H.264 encode entry point is a
    // definite absence; `vainfo` also exits non-zero when nothing initializes.
    if encoder {
        Verdict::Observed(())
    } else {
        Verdict::Absent
    }
}

/// Whether a PipeWire daemon socket exists in the runtime directory.
fn observe_pipewire() -> Verdict<()> {
    let runtime = match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(dir) => std::path::PathBuf::from(dir),
        None => return Verdict::Unverified("XDG_RUNTIME_DIR is not set".into()),
    };
    if runtime.join("pipewire-0").exists() {
        Verdict::Observed(())
    } else {
        Verdict::Absent
    }
}

/// Whether the session bus offers the desktop portal, which is how the caster
/// obtains a virtual source without a compositor scope.
fn observe_screencast_portal() -> Verdict<()> {
    let session = match zbus::blocking::Connection::session() {
        Ok(connection) => connection,
        Err(error) => return Verdict::Unverified(format!("session bus unavailable: {error}")),
    };
    // Asking the bus rather than activating the portal keeps the probe inert.
    let reply = match session.call_method(
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        Some("org.freedesktop.DBus"),
        "NameHasOwner",
        &("org.freedesktop.portal.Desktop",),
    ) {
        Ok(reply) => reply,
        Err(error) => return Verdict::Unverified(format!("session bus query failed: {error}")),
    };
    match reply.body().deserialize::<bool>() {
        Ok(true) => Verdict::Observed(()),
        Ok(false) => Verdict::Absent,
        Err(error) => Verdict::Unverified(format!("session bus reply unreadable: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_service_reads_as_absent_and_a_refusal_reads_as_unknown() {
        // The distinction the whole probe rests on: iwd not running is a fact;
        // being refused the bus is not.
        assert!(matches!(
            verdict_from_error::<()>(IwdError::ServiceUnavailable),
            Verdict::Absent
        ));
        assert!(matches!(
            verdict_from_error::<()>(IwdError::Unsupported("p2p".into())),
            Verdict::Absent
        ));
        assert!(matches!(
            verdict_from_error::<()>(IwdError::NoAdapter),
            Verdict::Absent
        ));
        assert!(matches!(
            verdict_from_error::<()>(IwdError::NotAuthorized("denied".into())),
            Verdict::Unverified(_)
        ));
        assert!(matches!(
            verdict_from_error::<()>(IwdError::Protocol("odd".into())),
            Verdict::Unverified(_)
        ));
        assert!(matches!(
            verdict_from_error::<()>(IwdError::BusUnavailable("no socket".into())),
            Verdict::Unverified(_)
        ));
    }
}
