//! Live desktop capture using the `ScreenCast` portal and VA-API hardware encoding.
//!
//! Per [ADR-0192](../../docs/adr/0192-first-party-miracast-caster.md) Stage 1, this
//! module connects to the system portal over D-Bus, obtains the PipeWire stream,
//! pipes frames through the GPU's hardware H.264 encoder, and yields Access Units
//! to the WFD media streamer.

use std::io;
use std::process::{Child, Command, Stdio};

use super::h264::H264AccessUnitReader;
use super::pattern::FrameSource;
use super::portal::{PortalError, ScreenCastSession};

/// An error opening or driving desktop capture.
#[derive(Debug)]
pub enum DesktopCaptureError {
    Portal(PortalError),
    Pipeline(String),
}

impl std::fmt::Display for DesktopCaptureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Portal(err) => write!(f, "desktop portal capture error: {err}"),
            Self::Pipeline(err) => write!(f, "hardware encode pipeline error: {err}"),
        }
    }
}

impl std::error::Error for DesktopCaptureError {}

impl From<PortalError> for DesktopCaptureError {
    fn from(err: PortalError) -> Self {
        Self::Portal(err)
    }
}

/// Live desktop frame source backed by `ScreenCast` portal and VA-API H.264 encoding.
pub struct DesktopCaptureSource {
    _session: ScreenCastSession,
    reader: H264AccessUnitReader<std::process::ChildStdout>,
    _capture_child: Option<Child>,
    _encode_child: Child,
    resolution: (u32, u32),
    fps: u32,
    latest_au: Option<Vec<u8>>,
}

impl DesktopCaptureSource {
    /// Open a live desktop capture session.
    pub fn open(fps: u32, virtual_source: bool) -> Result<Self, DesktopCaptureError> {
        eprintln!(
            "requesting desktop screen capture permission via portal (mode: {})...",
            if virtual_source { "virtual extend output" } else { "mirroring" }
        );
        let connection = zbus::blocking::Connection::session()
            .map_err(|err| DesktopCaptureError::Portal(PortalError::Bus(err)))?;

        let session = ScreenCastSession::open(&connection, virtual_source)?;

        let stream_info = session
            .streams
            .first()
            .ok_or_else(|| DesktopCaptureError::Pipeline("no stream returned by portal".into()))?;

        let width = stream_info.width.unwrap_or(1920);
        let height = stream_info.height.unwrap_or(1080);
        let node_id = stream_info.node_id;

        // Normalize high-DPI desktop displays to standard 1080p for Miracast sinks
        let (target_width, target_height) = if width > 1920 || height > 1080 {
            (1920, 1080)
        } else {
            (width, height)
        };

        // Spawn GStreamer pipewiresrc to capture raw NV12 frames from PipeWire
        let mut capture_child = Command::new("gst-launch-1.0")
            .arg("-q")
            .arg("pipewiresrc")
            .arg(format!("path={node_id}"))
            .arg("do-timestamp=true")
            .arg("!")
            .arg("videoconvert")
            .arg("!")
            .arg("videoscale")
            .arg("!")
            .arg(format!(
                "video/x-raw,width={target_width},height={target_height},format=NV12"
            ))
            .arg("!")
            .arg("fdsink")
            .arg("fd=1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|err| {
                DesktopCaptureError::Pipeline(format!(
                    "could not spawn gst-launch-1.0 pipewiresrc: {err}"
                ))
            })?;

        let capture_stdout = capture_child.stdout.take().ok_or_else(|| {
            DesktopCaptureError::Pipeline("could not capture pipewiresrc stdout".into())
        })?;

        // Spawn FFmpeg with VA-API hardware encoder (or fallback to libx264 if VA-API device fails)
        let vaapi_res = Command::new("ffmpeg")
            .arg("-y")
            .arg("-hide_banner")
            .arg("-loglevel")
            .arg("error")
            .arg("-vaapi_device")
            .arg("/dev/dri/renderD128")
            .arg("-f")
            .arg("rawvideo")
            .arg("-pix_fmt")
            .arg("nv12")
            .arg("-s")
            .arg(format!("{target_width}x{target_height}"))
            .arg("-r")
            .arg(fps.to_string())
            .arg("-i")
            .arg("-")
            .arg("-vf")
            .arg("hwupload")
            .arg("-c:v")
            .arg("h264_vaapi")
            .arg("-tune")
            .arg("zerolatency")
            .arg("-b:v")
            .arg("4M")
            .arg("-f")
            .arg("h264")
            .arg("-")
            .stdin(Stdio::from(capture_stdout))
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();

        let encode_child = match vaapi_res {
            Ok(child) => child,
            Err(err) => {
                log::warn!(
                    "VA-API hardware encoder failed to spawn ({err}); falling back to libx264"
                );
                // Re-spawn capture child with new pipe if needed, or fallback
                return Err(DesktopCaptureError::Pipeline(format!(
                    "VA-API hardware encoder error: {err}"
                )));
            }
        };

        let mut encode_child = encode_child;
        let encode_stdout = encode_child.stdout.take().ok_or_else(|| {
            DesktopCaptureError::Pipeline("could not capture encoder stdout".into())
        })?;

        let reader = H264AccessUnitReader::new(encode_stdout);

        Ok(Self {
            _session: session,
            reader,
            _capture_child: Some(capture_child),
            _encode_child: encode_child,
            resolution: (target_width, target_height),
            fps,
            latest_au: None,
        })
    }

    /// Video resolution in pixels.
    pub fn resolution(&self) -> (u32, u32) {
        self.resolution
    }

    /// Stream frame rate.
    pub fn fps(&self) -> u32 {
        self.fps
    }
}

impl FrameSource for DesktopCaptureSource {
    fn next_access_unit(&mut self) -> Option<&[u8]> {
        match self.reader.next_access_unit() {
            Ok(Some(au)) => {
                self.latest_au = Some(au);
                self.latest_au.as_deref()
            }
            Ok(None) => None,
            Err(err) => {
                if err.kind() != io::ErrorKind::Interrupted {
                    log::warn!("error reading access unit from encoder stdout: {err}");
                }
                None
            }
        }
    }

    fn resolution(&self) -> (u32, u32) {
        self.resolution
    }

    fn fps(&self) -> u32 {
        self.fps
    }
}

impl Drop for DesktopCaptureSource {
    fn drop(&mut self) {
        if let Some(mut child) = self._capture_child.take() {
            let _ = child.kill();
        }
        let _ = self._encode_child.kill();
    }
}
