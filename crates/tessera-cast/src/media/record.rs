//! Screen recording using the `ScreenCast` portal and hardware encoding.
//!
//! Shares the exact same `ScreenCastSession` and PipeWire stream substrate with
//! desktop casting (ADR-0192). Frames captured from PipeWire can be recorded
//! directly to an output file (e.g. MP4, MKV, TS), supporting both physical
//! monitor mirroring (`SOURCE_TYPE_MONITOR`) and session-scoped virtual extend
//! outputs (`SOURCE_TYPE_VIRTUAL`, ADR-0186).

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use super::portal::{PortalError, ScreenCastSession};

/// An error opening or driving a desktop screen recording session.
#[derive(Debug)]
pub enum RecordError {
    Portal(PortalError),
    Pipeline(String),
    Io(std::io::Error),
    InvalidOutput(String),
}

impl std::fmt::Display for RecordError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Portal(err) => write!(f, "record portal error: {err}"),
            Self::Pipeline(err) => write!(f, "record pipeline error: {err}"),
            Self::Io(err) => write!(f, "record IO error: {err}"),
            Self::InvalidOutput(err) => write!(f, "invalid output path: {err}"),
        }
    }
}

impl std::error::Error for RecordError {}

impl From<PortalError> for RecordError {
    fn from(err: PortalError) -> Self {
        Self::Portal(err)
    }
}

impl From<std::io::Error> for RecordError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

/// Configuration options for recording screen frames to a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordOptions {
    /// Path where the video file will be saved (e.g. `recording.mp4`, `screen.mkv`).
    pub output_path: PathBuf,
    /// Target frame rate in FPS (e.g. 30, 60).
    pub fps: u32,
    /// Whether to request a clean, session-scoped virtual extend output (`SOURCE_TYPE_VIRTUAL`, ADR-0186)
    /// rather than mirroring the physical display (`SOURCE_TYPE_MONITOR`).
    pub virtual_source: bool,
    /// Optional recording duration. If `None`, records until interrupted (Ctrl+C / stop signal).
    pub duration: Option<Duration>,
}

impl Default for RecordOptions {
    fn default() -> Self {
        Self {
            output_path: PathBuf::from("recording.mp4"),
            fps: 30,
            virtual_source: false,
            duration: None,
        }
    }
}

/// Final summary report of a completed recording session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordReport {
    pub output_path: PathBuf,
    pub resolution: (u32, u32),
    pub fps: u32,
    pub duration: Duration,
    pub bytes_written: u64,
}

/// Record the desktop screen to a video file using the ScreenCast portal.
///
/// If `options.virtual_source` is true, requests a virtual extend output (`SOURCE_TYPE_VIRTUAL`),
/// allowing isolated clean recording without physical display clutter.
pub fn record_screen(
    options: RecordOptions,
    stop_signal: Option<Arc<AtomicBool>>,
) -> Result<RecordReport, RecordError> {
    if options.output_path.as_os_str().is_empty() {
        return Err(RecordError::InvalidOutput("output path is empty".into()));
    }

    if let Some(parent) = options.output_path.parent()
        && !parent.as_os_str().is_empty()
        && !parent.exists()
    {
        std::fs::create_dir_all(parent)?;
    }

    eprintln!(
        "tessera-cast: requesting desktop capture permission via portal (mode: {})...",
        if options.virtual_source {
            "virtual extend output (ADR-0186)"
        } else {
            "physical monitor mirroring"
        }
    );

    let connection = zbus::blocking::Connection::session()
        .map_err(|err| RecordError::Portal(PortalError::Bus(err)))?;

    // 1. Open Portal ScreenCast session
    let session = ScreenCastSession::open(&connection, options.virtual_source)?;

    let stream_info = session
        .streams
        .first()
        .ok_or_else(|| RecordError::Pipeline("no stream returned by portal".into()))?;

    let width = stream_info.width.unwrap_or(1920);
    let height = stream_info.height.unwrap_or(1080);
    let node_id = stream_info.node_id;

    eprintln!(
        "tessera-cast: portal session active on PipeWire node {node_id} ({width}x{height} @ {}fps)",
        options.fps
    );

    // 2. Spawn GStreamer pipewiresrc to capture raw NV12 frames from PipeWire
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
            "video/x-raw,width={width},height={height},format=NV12"
        ))
        .arg("!")
        .arg("fdsink")
        .arg("fd=1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| {
            RecordError::Pipeline(format!("could not spawn gst-launch-1.0 pipewiresrc: {err}"))
        })?;

    let capture_stdout = capture_child
        .stdout
        .take()
        .ok_or_else(|| RecordError::Pipeline("could not capture pipewiresrc stdout".into()))?;

    // 3. Spawn FFmpeg encoding directly to the output file (VA-API hardware encoder with libx264 fallback)
    let start_time = Instant::now();
    let mut encode_child = spawn_ffmpeg_encoder(
        capture_stdout.into(),
        width,
        height,
        options.fps,
        &options.output_path,
        options.duration,
    )?;

    // 4. Wait for recording to complete, either by duration, stop signal, or child exit
    let mut stopped_manually = false;
    let poll_interval = Duration::from_millis(100);

    loop {
        // Check if child exited on its own
        if let Ok(Some(status)) = encode_child.try_wait() {
            if !status.success() {
                eprintln!("tessera-cast: encoder process exited with: {status}");
            }
            break;
        }

        // Check external stop signal
        if let Some(ref stop) = stop_signal
            && stop.load(Ordering::Relaxed)
        {
            stopped_manually = true;
            break;
        }

        // Check duration limit
        if let Some(duration) = options.duration
            && start_time.elapsed() >= duration
        {
            stopped_manually = true;
            break;
        }

        std::thread::sleep(poll_interval);
    }

    if stopped_manually {
        // Kill the capture child to close ffmpeg's stdin pipe,
        // signaling EOF so it flushes and cleanly writes the container trailer.
        let _ = capture_child.kill();

        // Wait up to 5 seconds for encoder to finalize
        let finalize_deadline = Instant::now() + Duration::from_secs(5);
        let mut exited = false;
        while Instant::now() < finalize_deadline {
            if let Ok(Some(_)) = encode_child.try_wait() {
                exited = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }

        if !exited {
            let _ = encode_child.kill();
        }
    } else {
        let _ = capture_child.kill();
        let _ = encode_child.wait();
    }

    let elapsed = start_time.elapsed();

    // Check resulting file size
    let bytes_written = std::fs::metadata(&options.output_path)
        .map(|m| m.len())
        .unwrap_or(0);

    eprintln!(
        "tessera-cast: recording completed: {} ({bytes_written} bytes, {:.1}s)",
        options.output_path.display(),
        elapsed.as_secs_f64()
    );

    // Keep session alive until here so virtual output survives until pipeline teardown
    drop(session);

    Ok(RecordReport {
        output_path: options.output_path,
        resolution: (width, height),
        fps: options.fps,
        duration: elapsed,
        bytes_written,
    })
}

fn spawn_ffmpeg_encoder(
    stdin: Stdio,
    width: u32,
    height: u32,
    fps: u32,
    output_path: &Path,
    duration: Option<Duration>,
) -> Result<Child, RecordError> {
    let has_vaapi = Path::new("/dev/dri/renderD128").exists();
    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-y")
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("warning");

    if has_vaapi {
        cmd.arg("-vaapi_device")
            .arg("/dev/dri/renderD128")
            .arg("-f")
            .arg("rawvideo")
            .arg("-pix_fmt")
            .arg("nv12")
            .arg("-s")
            .arg(format!("{width}x{height}"))
            .arg("-r")
            .arg(fps.to_string())
            .arg("-i")
            .arg("-")
            .arg("-vf")
            .arg("hwupload")
            .arg("-c:v")
            .arg("h264_vaapi")
            .arg("-b:v")
            .arg("8M");
    } else {
        log::info!("no /dev/dri/renderD128 device; using libx264 software encoder");
        cmd.arg("-f")
            .arg("rawvideo")
            .arg("-pix_fmt")
            .arg("nv12")
            .arg("-s")
            .arg(format!("{width}x{height}"))
            .arg("-r")
            .arg(fps.to_string())
            .arg("-i")
            .arg("-")
            .arg("-c:v")
            .arg("libx264")
            .arg("-preset")
            .arg("veryfast")
            .arg("-crf")
            .arg("22");
    }

    if let Some(duration) = duration {
        cmd.arg("-t").arg(format!("{:.3}", duration.as_secs_f64()));
    }

    cmd.arg(output_path);
    cmd.stdin(stdin);
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::inherit());

    cmd.spawn()
        .map_err(|e| RecordError::Pipeline(format!("could not spawn ffmpeg encoder: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_options_default() {
        let opt = RecordOptions::default();
        assert_eq!(opt.output_path, PathBuf::from("recording.mp4"));
        assert_eq!(opt.fps, 30);
        assert!(!opt.virtual_source);
        assert_eq!(opt.duration, None);
    }

    #[test]
    fn record_empty_path_fails_validation() {
        let opt = RecordOptions {
            output_path: PathBuf::new(),
            ..Default::default()
        };
        let res = record_screen(opt, None);
        assert!(matches!(res, Err(RecordError::InvalidOutput(_))));
    }
}
