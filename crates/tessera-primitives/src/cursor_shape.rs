//! The `wp_cursor_shape_device_v1` shape vocabulary and its XDG cursor names.
//!
//! One table, two consumers: the compositor resolves the physical user's cursor
//! from it, and the shell resolves the Agent seat's cursor from it. The two were
//! previously verbatim copies of the same 40-line match, which is exactly the
//! kind of duplication that drifts silently — a shape added to one and not the
//! other renders as the wrong arrow with no error anywhere.
//!
//! Protocol shape values are stable: they come from the Wayland
//! `cursor-shape-v1` XML, so a numeric literal here is the protocol's own
//! number, not a local convention.

/// XDG cursor file names for a `wp_cursor_shape_device_v1` shape, in preference
/// order: the protocol/CSS name first, then the legacy X11 aliases that real
/// themes ship.
///
/// The first name the theme actually provides wins, so a theme that only carries
/// `left_ptr` still paints `default`.
pub fn xdg_names(shape: u32) -> &'static [&'static str] {
    match shape {
        1 => &["default", "left_ptr", "arrow"],
        2 => &["context-menu", "left_ptr"],
        3 => &["help", "question_arrow", "left_ptr_help", "left_ptr"],
        4 => &["pointer", "hand2", "pointing_hand", "left_ptr"],
        5 => &["progress", "left_ptr_watch", "watch"],
        6 => &["wait", "watch", "left_ptr"],
        7 => &["cell", "crosshair"],
        8 => &["crosshair", "cross"],
        9 => &["text", "xterm", "ibeam"],
        10 => &["vertical-text", "xterm"],
        11 => &["alias", "dnd-link", "left_ptr"],
        12 => &["copy", "dnd-copy", "left_ptr"],
        13 => &["move", "dnd-move", "fleur", "all-scroll"],
        14 => &["no-drop", "not-allowed"],
        15 => &["not-allowed", "forbidden", "crossed_circle"],
        16 => &["grab", "hand1", "openhand", "left_ptr"],
        17 => &["grabbing", "closedhand", "hand1"],
        18 => &["e-resize", "right_side", "sb_h_double_arrow"],
        19 => &["n-resize", "top_side", "sb_v_double_arrow"],
        20 => &["ne-resize", "top_right_corner", "sb_h_double_arrow"],
        21 => &["nw-resize", "top_left_corner", "sb_h_double_arrow"],
        22 => &["s-resize", "bottom_side", "sb_v_double_arrow"],
        23 => &["se-resize", "bottom_right_corner", "sb_h_double_arrow"],
        24 => &["sw-resize", "bottom_left_corner", "sb_h_double_arrow"],
        25 => &["w-resize", "left_side", "sb_h_double_arrow"],
        26 => &["ew-resize", "sb_h_double_arrow", "h_double_arrow"],
        27 => &["ns-resize", "sb_v_double_arrow", "v_double_arrow"],
        28 => &["nesw-resize", "bd_double_arrow", "size_bdiag"],
        29 => &["nwse-resize", "fd_double_arrow", "size_fdiag"],
        30 => &["col-resize", "sb_h_double_arrow", "h_double_arrow"],
        31 => &["row-resize", "sb_v_double_arrow", "v_double_arrow"],
        32 => &["all-scroll", "fleur", "move"],
        33 => &["zoom-in", "zoom_in"],
        34 => &["zoom-out", "zoom_out"],
        35 => &["dnd-ask", "question_arrow", "help"],
        36 => &["all-resize", "all-scroll", "fleur", "move"],
        _ => &["default", "left_ptr", "arrow"],
    }
}

/// `wp_cursor_shape_device_v1.shape` value `default` (the protocol's 1).
pub const DEFAULT_SHAPE: u32 = 1;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_protocol_shape_resolves_to_at_least_one_name() {
        for shape in 1..=36 {
            assert!(!xdg_names(shape).is_empty(), "shape {shape}");
        }
    }

    #[test]
    fn protocol_names_come_first() {
        assert_eq!(xdg_names(DEFAULT_SHAPE)[0], "default");
        assert_eq!(xdg_names(4)[0], "pointer");
        assert_eq!(xdg_names(9)[0], "text");
        assert_eq!(xdg_names(26)[0], "ew-resize");
        assert_eq!(xdg_names(36)[0], "all-resize");
    }

    #[test]
    fn unknown_shapes_fall_back_to_the_arrow() {
        // A client may send a shape from a newer protocol revision; the answer
        // is the default arrow, not a missing cursor.
        assert_eq!(xdg_names(0), xdg_names(DEFAULT_SHAPE));
        assert_eq!(xdg_names(9999), xdg_names(DEFAULT_SHAPE));
    }
}
