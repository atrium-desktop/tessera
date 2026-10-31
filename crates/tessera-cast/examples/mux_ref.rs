//! Diagnostic: mux the reference AAC with our own TsMuxer.

use tessera_cast::wfd::ts::{TsLayout, TsMuxer};

fn split_adts(data: &[u8]) -> Vec<Vec<u8>> {
    let mut frames = Vec::new();
    let mut offset = 0;
    while offset + 7 <= data.len() {
        if data[offset] != 0xff || data[offset + 1] & 0xf6 != 0xf0 {
            offset += 1;
            continue;
        }
        let header = if data[offset + 1] & 1 == 1 { 7 } else { 9 };
        let len = (usize::from(data[offset + 3] & 3) << 11)
            | (usize::from(data[offset + 4]) << 3)
            | usize::from(data[offset + 5] >> 5);
        if len < header || offset + len > data.len() {
            offset += 1;
            continue;
        }
        frames.push(data[offset..offset + len].to_vec());
        offset += len;
    }
    frames
}

fn main() {
    let audio = std::fs::read("/tmp/ref.aac").expect("read reference AAC");
    let video = std::fs::read("/tmp/small.h264").expect("read low-bitrate video");
    let frames = split_adts(&audio);
    assert!(!frames.is_empty(), "reference AAC has no ADTS frames");
    let mut muxer = TsMuxer::with_audio(TsLayout::default());
    let mut out = Vec::new();
    let mut audio_index = 0usize;
    let mut audio_pts = 0u64;
    let mut group = Vec::new();
    let mut group_pts = 0u64;
    let mut group_frames = 0usize;
    for frame in 0..900u64 {
        if frame % 3 == 0 {
            out.extend_from_slice(&muxer.pat());
            out.extend_from_slice(&muxer.pmt());
        }
        let pcr = if frame % 3 == 0 {
            Some(frame * 3000)
        } else {
            None
        };
        out.extend_from_slice(&muxer.video_access_unit_with_pcr(&video, frame * 3000, pcr));
        while audio_index < frames.len() && audio_pts <= (frame + 1) * 3000 {
            if group_frames == 0 {
                group_pts = audio_pts;
            }
            group.extend_from_slice(&frames[audio_index]);
            group_frames += 1;
            audio_index += 1;
            audio_pts += 1920;
            if group_frames == 8 {
                out.extend_from_slice(&muxer.audio_access_unit(&group, group_pts));
                group.clear();
                group_frames = 0;
            }
        }
    }
    if group_frames > 0 {
        out.extend_from_slice(&muxer.audio_access_unit(&group, group_pts));
    }
    std::fs::write("/tmp/ours_low.ts", out).expect("write our TS");
    eprintln!(
        "muxed {} audio frames, {} bytes",
        audio_index,
        std::fs::metadata("/tmp/ours_low.ts")
            .map(|m| m.len())
            .unwrap_or(0)
    );
}
