//! AAC audio capture from PipeWire for WFD.
//!
//! WFD carries audio as a second elementary stream inside the same MPEG2-TS as
//! video. PipeWire already owns the host audio graph, so this module taps the
//! default sink's monitor (system output) or the default source (microphone),
//! asks `ffmpeg` for AAC-LC ADTS, and exposes complete ADTS frames with a 90 kHz
//! presentation timestamp. The TS muxer and RTP packetizer are unchanged: both
//! streams travel in one transport stream and one RTP flow.
//!
//! Audio is opt-in. Capturing the system output is a privacy decision, so the
//! configuration default is off and the CLI requires an explicit `--audio`.

use std::io::{BufReader, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

/// WFD AAC-LC is negotiated at 48 kHz stereo.
pub const SAMPLE_RATE: u32 = 48_000;
const NINETY_KHZ: u64 = 90_000;
const AAC_SAMPLES_PER_BLOCK: u64 = 1024;

/// Which PipeWire node to tap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioSourceKind {
    /// The default sink's monitor — the audio the desktop is playing.
    System,
    /// The default source — the microphone.
    Microphone,
}

impl AudioSourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Microphone => "microphone",
        }
    }

    fn selector(self) -> &'static str {
        match self {
            Self::System => "@DEFAULT_AUDIO_SINK@",
            Self::Microphone => "@DEFAULT_AUDIO_SOURCE@",
        }
    }

    /// Properties that make the node this kind names actually *capture* what it
    /// claims to capture.
    ///
    /// A sink is an output-direction node, so pointing `pw-record --target` at
    /// it is not enough: PipeWire links the capture stream to a source, and
    /// without `stream.capture.sink=true` the link lands on the default
    /// *microphone* instead. The stream then records the room, not the desktop,
    /// which is the opposite of what `system` promises — and a privacy
    /// surprise, not a cosmetic defect.
    fn capture_properties(self) -> &'static [&'static str] {
        match self {
            Self::System => &["stream.capture.sink=true"],
            Self::Microphone => &[],
        }
    }
}

/// One encoded ADTS frame and its 90 kHz presentation timestamp.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioFrame {
    pub data: Vec<u8>,
    pub pts_90k: u64,
}

/// A non-blocking source of encoded audio frames.
pub trait AudioFrameSource: Send {
    /// Return the next frame if one is ready. Never blocks.
    fn try_next(&mut self) -> Option<AudioFrame>;

    /// A terminal capture/encode failure, if one occurred.
    fn failure(&mut self) -> Option<String> {
        None
    }
}

/// A live `pw-record` → `ffmpeg` pipeline producing AAC ADTS frames.
pub struct AacCapture {
    record: Child,
    encode: Child,
    frames: Receiver<AudioFrame>,
    failure: Arc<Mutex<Option<String>>>,
}

impl AacCapture {
    /// Start capturing `kind` and encoding AAC-LC at `bitrate_kbps`.
    pub fn start(kind: AudioSourceKind, bitrate_kbps: u32) -> Result<Self, String> {
        let target = default_node_id(kind.selector())?;
        let bitrate = format!("{bitrate_kbps}k");
        let mut record = Command::new("pw-record")
            .args([
                "--target",
                &target,
                "--raw",
                "--rate",
                "48000",
                "--channels",
                "2",
                "--format",
                "s16",
                "--latency",
                "100ms",
            ])
            // `-P` is repeatable; one flag per property keeps the spawn
            // argument list a plain `&str` slice without a formatted join.
            .args(
                kind.capture_properties()
                    .iter()
                    .flat_map(|property| ["-P", property]),
            )
            .arg("-")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("could not start pw-record: {error}"))?;
        let Some(record_stdout) = record.stdout.take() else {
            let _ = record.kill();
            let _ = record.wait();
            return Err("pw-record did not expose its stdout".to_owned());
        };

        let mut encode = match Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "s16le",
                "-ar",
                "48000",
                "-ac",
                "2",
                "-i",
                "pipe:0",
                "-c:a",
                "aac",
                "-b:a",
                &bitrate,
                "-f",
                "adts",
                "pipe:1",
            ])
            .stdin(Stdio::from(record_stdout))
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => child,
            Err(error) => {
                let _ = record.kill();
                let _ = record.wait();
                return Err(format!("could not start ffmpeg: {error}"));
            }
        };
        let Some(encode_stdout) = encode.stdout.take() else {
            let _ = encode.kill();
            let _ = record.kill();
            let _ = encode.wait();
            let _ = record.wait();
            return Err("ffmpeg did not expose its stdout".to_owned());
        };

        let (sender, frames) = std::sync::mpsc::channel();
        let failure = Arc::new(Mutex::new(None));
        let thread_failure = Arc::clone(&failure);
        thread::Builder::new()
            .name("tessera-cast-audio".to_owned())
            .spawn(move || read_adts(encode_stdout, sender, thread_failure))
            .map_err(|error| format!("could not start the audio reader: {error}"))?;

        Ok(Self {
            record,
            encode,
            frames,
            failure,
        })
    }

    /// The PipeWire sample rate the pipeline was started with.
    pub fn sample_rate(&self) -> u32 {
        SAMPLE_RATE
    }
}

impl AudioFrameSource for AacCapture {
    fn try_next(&mut self) -> Option<AudioFrame> {
        self.frames.try_recv().ok()
    }

    fn failure(&mut self) -> Option<String> {
        if let Some(reason) = self.failure.lock().ok().and_then(|slot| slot.clone()) {
            return Some(reason);
        }
        if let Ok(Some(status)) = self.record.try_wait()
            && !status.success()
        {
            return Some(format!("pw-record exited: {status}"));
        }
        if let Ok(Some(status)) = self.encode.try_wait()
            && !status.success()
        {
            return Some(format!("ffmpeg exited: {status}"));
        }
        None
    }
}

impl Drop for AacCapture {
    fn drop(&mut self) {
        // The children hold the PipeWire and encoder resources; stop both and
        // reap them so a finished session leaves no stray process behind.
        let _ = self.encode.kill();
        let _ = self.record.kill();
        let _ = self.encode.wait();
        let _ = self.record.wait();
    }
}

fn default_node_id(selector: &str) -> Result<String, String> {
    let output = Command::new("wpctl")
        .args(["inspect", selector])
        .output()
        .map_err(|error| format!("could not run wpctl: {error}"))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "wpctl inspect {selector} failed: {}",
            detail.trim()
        ));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let first = text.lines().next().unwrap_or_default();
    first
        .strip_prefix("id ")
        .and_then(|rest| rest.split(',').next())
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| format!("could not parse the node id from `wpctl inspect {selector}`"))
}

fn read_adts(output: impl Read, sender: Sender<AudioFrame>, failure: Arc<Mutex<Option<String>>>) {
    let mut reader = BufReader::new(output);
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    let mut pts_90k = 0u64;
    let mut dump = std::env::var_os("TESSERA_CAST_DUMP_AUDIO")
        .and_then(|path| std::fs::File::create(path).ok());
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => buffer.extend_from_slice(&chunk[..read]),
            Err(error) => {
                if let Ok(mut slot) = failure.lock() {
                    *slot = Some(format!("audio reader failed: {error}"));
                }
                return;
            }
        }
        while let Some((frame, samples, rate)) = take_adts_frame(&mut buffer) {
            if let Some(file) = dump.as_mut() {
                let _ = file.write_all(&frame);
            }
            if sender
                .send(AudioFrame {
                    data: frame,
                    pts_90k,
                })
                .is_err()
            {
                return;
            }
            pts_90k += samples * NINETY_KHZ / u64::from(rate);
        }
    }
}

/// Extract one complete ADTS frame. Returns its bytes, its sample count, and
/// the sample rate. Resynchronises on a bad header instead of trusting the
/// encoder's framing blindly.
fn take_adts_frame(buffer: &mut Vec<u8>) -> Option<(Vec<u8>, u64, u32)> {
    const RATES: [u32; 13] = [
        96_000, 88_200, 64_000, 48_000, 44_100, 32_000, 24_000, 22_050, 16_000, 12_000, 11_025,
        8_000, 7_350,
    ];
    loop {
        let start = buffer
            .windows(2)
            .position(|pair| pair[0] == 0xff && pair[1] & 0xf6 == 0xf0)?;
        if start > 0 {
            buffer.drain(..start);
        }
        if buffer.len() < 7 {
            return None;
        }
        let header_len = if buffer[1] & 0x01 == 1 { 7 } else { 9 };
        let rate_index = usize::from((buffer[2] >> 2) & 0x0f);
        if rate_index >= RATES.len() {
            buffer.remove(0);
            continue;
        }
        let frame_len = (usize::from(buffer[3] & 0x03) << 11)
            | (usize::from(buffer[4]) << 3)
            | usize::from(buffer[5] >> 5);
        if frame_len < header_len {
            buffer.remove(0);
            continue;
        }
        if buffer.len() < frame_len {
            return None;
        }
        let blocks = u64::from((buffer[6] & 0x03) + 1);
        let frame = buffer[..frame_len].to_vec();
        buffer.drain(..frame_len);
        return Some((frame, AAC_SAMPLES_PER_BLOCK * blocks, RATES[rate_index]));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adts_frame(payload: &[u8]) -> Vec<u8> {
        let frame_len = 7 + payload.len();
        let mut frame = vec![0u8; 7];
        frame[0] = 0xff;
        frame[1] = 0xf1; // MPEG-4, layer 0, no CRC
        frame[2] = 0x40 | 0x0c; // AAC-LC, 48 kHz (index 3) in bits 5..2
        frame[3] = 0x80 | ((frame_len >> 11) as u8 & 0x03);
        frame[4] = (frame_len >> 3) as u8;
        frame[5] = ((frame_len as u8 & 0x07) << 5) | 0x1f;
        frame[6] = 0xfc; // buffer fullness high bits + one raw block
        frame.extend_from_slice(payload);
        frame
    }

    #[test]
    fn an_adts_frame_is_returned_with_its_samples_and_rate() {
        let mut buffer = adts_frame(&[1, 2, 3, 4]);
        let (frame, samples, rate) = take_adts_frame(&mut buffer).expect("frame");
        assert_eq!(frame.len(), 11);
        assert_eq!(samples, 1024);
        assert_eq!(rate, 48_000);
        assert!(buffer.is_empty());
    }

    #[test]
    fn a_partial_frame_waits_for_more_bytes() {
        let complete = adts_frame(&[9; 10]);
        let mut buffer = complete[..complete.len() - 2].to_vec();
        assert!(take_adts_frame(&mut buffer).is_none());
        buffer.extend_from_slice(&complete[complete.len() - 2..]);
        assert!(take_adts_frame(&mut buffer).is_some());
    }

    #[test]
    fn garbage_before_the_sync_word_is_discarded() {
        let mut buffer = vec![0x00, 0x11, 0x22];
        buffer.extend_from_slice(&adts_frame(&[7; 5]));
        assert!(take_adts_frame(&mut buffer).is_some());
    }

    #[test]
    fn a_bad_rate_index_resynchronises_instead_of_yielding_a_frame() {
        let mut bad = adts_frame(&[0; 4]);
        bad[2] = 0x40 | (0x0f << 2);
        let mut buffer = bad;
        assert!(take_adts_frame(&mut buffer).is_none());
    }

    #[test]
    fn the_system_source_asks_for_the_sink_monitor() {
        // `pw-record --target <sink>` alone records the default *source*: a
        // sink is output-direction, so PipeWire links the capture stream to a
        // microphone unless the stream declares that it wants the sink's
        // monitor. Getting this wrong records the room instead of the desktop,
        // which is both the wrong feature and a privacy surprise.
        assert_eq!(
            AudioSourceKind::System.capture_properties(),
            ["stream.capture.sink=true"],
            "system audio must capture the sink monitor, not the microphone"
        );
        // The microphone is already a source, so it needs no property and must
        // not be given one that would send it looking for a sink.
        assert!(AudioSourceKind::Microphone.capture_properties().is_empty());
    }
}
