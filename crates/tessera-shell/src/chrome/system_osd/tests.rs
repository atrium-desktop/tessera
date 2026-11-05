use super::*;
use crate::component::OsdKind;

fn battery(percent: u8, critical: bool) -> OsdParams {
    OsdParams {
        kind: OsdKind::Battery,
        title: format!("Battery {percent}%"),
        detail: None,
        level: Some(percent),
        critical,
    }
}

#[test]
fn an_osd_is_solid_through_its_hold_then_drains() {
    assert_eq!(SystemOsd::opacity_at(0), 1.0);
    assert_eq!(SystemOsd::opacity_at(OSD_HOLD_MS), 1.0);
    // Past the hold the surface drains, and it is gone at the window's end.
    let mid = SystemOsd::opacity_at(OSD_HOLD_MS + OSD_FADE_MS / 2);
    assert!(mid > 0.0 && mid < 1.0, "the exit fades, got {mid}");
    assert_eq!(SystemOsd::opacity_at(OSD_TTL_MS), 0.0);
    assert_eq!(
        SystemOsd::opacity_at(OSD_TTL_MS + 5_000),
        0.0,
        "a stale reading never comes back"
    );
}

#[test]
fn a_new_reading_replaces_the_current_one_instead_of_stacking() {
    let mut osd = SystemOsd::new();
    osd.now_ms = 1_000;
    osd.show(battery(40, false));
    osd.now_ms = 1_100;
    osd.show(battery(39, false));

    // Two steps in a row are one reading: the surface shows the latest value
    // and the newer raised instant, not two panels.
    let live = osd.live().expect("the newest reading is live");
    assert_eq!(live.params.level, Some(39));
    assert_eq!(osd.raised_ms, 1_100);
}

#[test]
fn the_reading_leaves_without_being_dismissed() {
    let mut osd = SystemOsd::new();
    osd.now_ms = 0;
    osd.show(battery(15, false));
    assert!(osd.osd_active());

    osd.now_ms = OSD_TTL_MS - 1;
    assert!(osd.osd_active(), "still inside its window");

    osd.now_ms = OSD_TTL_MS;
    assert!(!osd.osd_active(), "the window closed on its own");
    assert!(
        !osd.requires_composition(),
        "an expired OSD paints nothing, so it must not hold the frame loop"
    );
}

#[test]
fn the_osd_never_captures_input() {
    // The whole justification for a non-modal reading is that it cannot steal a
    // keystroke or a click. This is the surface's charter, pinned.
    let osd = SystemOsd::new();
    assert!(
        !osd.captures_keyboard(),
        "an OSD must never take the keyboard"
    );
    assert!(
        !osd.captures_pointer(0.0, 0.0, (1920.0, 1080.0), &[], &workspace_snapshot()),
        "an OSD must never take the pointer, even inside its own rect"
    );
    assert!(!osd.modal_active(), "an OSD is not a modal");
    assert!(
        !osd.exclusive_presentation_active(),
        "an OSD does not own the chrome band"
    );
}

#[test]
fn the_osd_holds_no_screen_space() {
    let osd = SystemOsd::new();
    assert_eq!(
        osd.reserved(),
        crate::Reserved::default(),
        "tiled windows run underneath the reading"
    );
}

#[test]
fn the_deadline_names_the_exact_end_of_the_window() {
    let mut osd = SystemOsd::new();
    osd.now_ms = 0;
    osd.show(battery(10, false));
    osd.now_ms = 600;
    assert_eq!(
        osd.animation_deadline_in(),
        Some(std::time::Duration::from_millis(OSD_TTL_MS - 600)),
        "the host schedules one wake instead of pumping frames"
    );

    // An expired reading has no deadline left to wait for.
    osd.now_ms = OSD_TTL_MS;
    assert_eq!(osd.animation_deadline_in(), None);
}

#[test]
fn the_osd_clears_the_dock_band_and_stays_centred() {
    let display = (1920.0, 1080.0);
    let rect = SystemOsd::rect_for(display);
    assert_eq!(
        rect.x + rect.w * 0.5,
        display.0 * 0.5,
        "the reading is centred on the output"
    );
    assert!(
        rect.y + rect.h < display.1,
        "the reading sits above the dock rather than over it"
    );
    // A narrow output shrinks the surface rather than letting it overflow.
    let narrow = SystemOsd::rect_for((240.0, 800.0));
    assert!(narrow.w <= 240.0 - OSD_BOTTOM_INSET);
}

#[test]
fn only_level_kinds_draw_a_gauge() {
    assert!(OsdKind::Battery.is_level());
    assert!(OsdKind::Volume.is_level());
    assert!(OsdKind::Brightness.is_level());
    assert!(!OsdKind::Wireless.is_level());
    assert!(!OsdKind::Bluetooth.is_level());
    // An output reading names a device; the level is still the volume fader,
    // so this kind draws no second gauge (ADR-0215).
    assert!(!OsdKind::Output.is_level());
}

fn workspace_snapshot() -> WorkspaceSnapshot {
    WorkspaceSnapshot {
        outputs: Vec::new(),
    }
}
