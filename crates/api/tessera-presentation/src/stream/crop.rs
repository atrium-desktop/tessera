//! Stream-frame crop and target-resolution geometry.

use tessera_model::Rect;

/// Extract one target region's rows out of a shared full-frame readback.
/// `rect` is in physical pixels and already clamped to the frame.
pub fn crop_stream_frame(width: u32, bgra: &[u8], rect: Rect) -> (u32, u32, std::sync::Arc<[u8]>) {
    let crop_width = rect.size.w.max(0) as u32;
    let crop_height = rect.size.h.max(0) as u32;
    let x = rect.origin.x as usize;
    let y = rect.origin.y as usize;
    let row = crop_width as usize * 4;
    let mut out = Vec::with_capacity(row * crop_height as usize);
    for line in y..y + crop_height as usize {
        let start = (line * width as usize + x) * 4;
        out.extend_from_slice(&bgra[start..start + row]);
    }
    (crop_width, crop_height, out.into())
}

/// One output's current rectangle in physical desktop-frame pixels,
/// resolved from the live model by connector name (IPC protocol 29). The
/// logical rect maps through the desktop render scale exactly like a
/// window's rect, so an output's rectangle always contains the windows the
/// compositor placed on it. `None` when no live output carries the name.
pub fn resolve_output_rect(
    outputs: &[tessera_model::output::OutputInfo],
    connector: &str,
    scale: f32,
    frame_width: u32,
    frame_height: u32,
) -> Option<Rect> {
    let output = outputs
        .iter()
        .find(|candidate| candidate.connector == connector)?;
    Some(crate::logical_rect_to_physical(
        output.geometry.logical_rect(),
        scale,
        frame_width,
        frame_height,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outputs() -> Vec<tessera_model::output::OutputInfo> {
        vec![
            tessera_model::output::OutputInfo {
                connector: "HDMI-A-1".into(),
                geometry: tessera_model::output::OutputGeometry {
                    mode: tessera_model::output::OutputMode {
                        width: 1920,
                        height: 1080,
                        refresh_mhz: 60_000,
                    },
                    scale: tessera_model::output::Scale(1.0),
                    transform: tessera_model::Transform::Normal,
                    logical_origin: tessera_model::Point { x: 0, y: 0 },
                },
                available_modes: Vec::new(),
                color_caps: tessera_model::edid::EdidColorCapabilities::default(),
            },
            tessera_model::output::OutputInfo {
                connector: "DP-1".into(),
                geometry: tessera_model::output::OutputGeometry {
                    mode: tessera_model::output::OutputMode {
                        width: 2560,
                        height: 1440,
                        refresh_mhz: 60_000,
                    },
                    scale: tessera_model::output::Scale(1.0),
                    transform: tessera_model::Transform::Normal,
                    logical_origin: tessera_model::Point { x: 1920, y: 0 },
                },
                available_modes: Vec::new(),
                color_caps: tessera_model::edid::EdidColorCapabilities::default(),
            },
        ]
    }

    #[test]
    fn resolve_output_rect_maps_and_clamps_by_connector() {
        let outputs = outputs();
        assert_eq!(
            resolve_output_rect(&outputs, "DP-1", 1.0, 4480, 1440),
            Some(Rect::new(1920, 0, 2560, 1440))
        );
        assert_eq!(
            resolve_output_rect(&outputs, "HDMI-A-1", 1.0, 4480, 1440),
            Some(Rect::new(0, 0, 1920, 1080))
        );
        // Render scale 2 maps the logical layout onto the physical frame.
        assert_eq!(
            resolve_output_rect(&outputs, "DP-1", 2.0, 8960, 2880),
            Some(Rect::new(3840, 0, 5120, 2880))
        );
        // Unknown connectors resolve to None (an error at stream start).
        assert_eq!(
            resolve_output_rect(&outputs, "USB-C-1", 1.0, 4480, 1440),
            None
        );
    }

    #[test]
    fn crop_stream_frame_extracts_rows() {
        // 4x2 frame, pixels valued by their index.
        let bgra: Vec<u8> = (0u8..32).collect();
        let (w, h, pixels) = crop_stream_frame(4, &bgra, Rect::new(1, 1, 2, 1));
        assert_eq!((w, h), (2, 1));
        // Row 1 starts at byte 16; two pixels from x=1: bytes 20..28.
        assert_eq!(&pixels[..], &(20u8..28).collect::<Vec<u8>>()[..]);
    }
}
