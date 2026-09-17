//! Parameterized placement geometry without shell styling.
use tessera_types::{Point, Rect, Size};

pub fn grid_with_spacing(area: Rect, count: usize, margin: i32, gap: i32) -> Vec<Rect> {
    if count == 0 || area.size.w <= 0 || area.size.h <= 0 {
        return Vec::new();
    }
    let columns = (count as f32).sqrt().ceil() as i32;
    let rows = (count as i32 + columns - 1) / columns;
    let inner = Rect::new(
        area.origin.x + margin,
        area.origin.y + margin,
        (area.size.w - 2 * margin).max(1),
        (area.size.h - 2 * margin).max(1),
    );
    let slot_w = ((inner.size.w - (columns - 1) * gap) / columns).max(1);
    let slot_h = ((inner.size.h - (rows - 1) * gap) / rows).max(1);
    // Center the used portion of the inner rect when the last row is short
    // and when the slots do not fill the area exactly.
    let used_w = columns * slot_w + (columns - 1) * gap;
    let used_h = rows * slot_h + (rows - 1) * gap;
    let start_x = inner.origin.x + (inner.size.w - used_w).max(0) / 2;
    let start_y = inner.origin.y + (inner.size.h - used_h).max(0) / 2;
    (0..count)
        .map(|i| {
            let col = i as i32 % columns;
            let row = i as i32 / columns;
            Rect::new(
                start_x + col * (slot_w + gap),
                start_y + row * (slot_h + gap),
                slot_w,
                slot_h,
            )
        })
        .collect()
}

pub fn fit(slot: Rect, content: Size) -> Rect {
    if content.w <= 0 || content.h <= 0 {
        return slot;
    }
    let scale = (slot.size.w as f32 / content.w as f32).min(slot.size.h as f32 / content.h as f32);
    let w = ((content.w as f32 * scale).round() as i32).max(1);
    let h = ((content.h as f32 * scale).round() as i32).max(1);
    Rect {
        origin: Point {
            x: slot.origin.x + (slot.size.w - w) / 2,
            y: slot.origin.y + (slot.size.h - h) / 2,
        },
        size: Size { w, h },
    }
}
