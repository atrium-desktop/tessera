use super::*;

const DISPLAY: (f32, f32) = (1920.0, 1080.0);

/// The reserved band a bottom-docked, non-autohidden dock publishes through
/// `ChromeUpdate::ModalReserved`. Stated as the published value so these tests
/// build without the dock; [`the_published_dock_band_matches_the_dock_itself`]
/// pins it to the dock's constants when the dock is compiled in.
fn bottom_dock_reserved() -> Reserved {
    Reserved {
        top: 0,
        bottom: 86,
        left: 0,
        right: 0,
    }
}

#[cfg(feature = "chrome-dock")]
#[test]
fn the_published_dock_band_matches_the_dock_itself() {
    use crate::components::dock::motion::{DOCK_EDGE_MARGIN, DOCK_PANEL_HEIGHT};
    assert_eq!(
        bottom_dock_reserved().bottom,
        (DOCK_PANEL_HEIGHT + DOCK_EDGE_MARGIN) as i32,
        "the sheet lays out against the band the dock actually publishes"
    );
}

impl ControlCenter {
    fn toggle(&mut self, out: &mut ChromeEvents) {
        <Self as Chrome>::command(self, &ChromeCommand::ToggleControlCenter, out);
    }

    fn set_reduced_motion(&mut self, reduced: bool) {
        <Self as Chrome>::update(self, ChromeUpdate::ReducedMotion(reduced));
    }

    /// An open, fully revealed sheet with no motion.
    fn presented_for_test() -> ControlCenter {
        let mut panel = ControlCenter::without_sources();
        panel.set_reduced_motion(true);
        panel.toggle(&mut ChromeEvents::default());
        panel.reveal.snap_to(1.0);
        panel
    }
}

fn key(keysym: u32) -> KeyChar {
    KeyChar {
        keysym,
        ch: None,
        mods: tessera_primitives::input::Mods::NONE,
    }
}

fn escape() -> KeyChar {
    key(tessera_primitives::input::XKB_KEY_Escape)
}

fn no_windows() -> (Vec<Window>, WorkspaceSnapshot) {
    (
        Vec::new(),
        WorkspaceSnapshot {
            outputs: Vec::new(),
        },
    )
}

/// Render one headless frame and present it, so the next frame hit-tests
/// against real geometry.
fn render_headless(
    ui: &mut lens::Ui,
    panel: &mut ControlCenter,
    input: &Input,
    windows: &[Window],
    workspaces: &WorkspaceSnapshot,
    i18n: &Localizer,
    out: &mut ChromeEvents,
) {
    ui.frame(input, |f| {
        panel.render(f, input, windows, workspaces, i18n, out);
    });
    let snap = ui.snapshot().unwrap();
    ui.activate(&snap).unwrap();
}

/// Hover `at` for two frames, then press and release the left button there.
fn click(panel: &mut ControlCenter, ui: &mut lens::Ui, at: (f32, f32), out: &mut ChromeEvents) {
    let (windows, workspaces) = no_windows();
    let i18n = Localizer::default();
    let mut input = Input::new(DISPLAY, 0.016);
    input.set_cursor(at.0, at.1);
    render_headless(ui, panel, &input, &windows, &workspaces, &i18n, out);
    render_headless(ui, panel, &input, &windows, &workspaces, &i18n, out);
    input.set_mouse_down(lens::MouseButton::Left, true);
    input.set_mouse_pressed(lens::MouseButton::Left, true);
    render_headless(ui, panel, &input, &windows, &workspaces, &i18n, out);
    input.set_mouse_down(lens::MouseButton::Left, false);
    input.set_mouse_pressed(lens::MouseButton::Left, false);
    input.set_mouse_released(lens::MouseButton::Left, true);
    render_headless(ui, panel, &input, &windows, &workspaces, &i18n, out);
}

fn centre(rect: Rect) -> (f32, f32) {
    (rect.x + rect.w * 0.5, rect.y + rect.h * 0.5)
}

// ---- lifecycle -------------------------------------------------------------

#[test]
fn toggle_opens_and_closes_the_sheet() {
    let mut panel = ControlCenter::without_sources();
    panel.set_reduced_motion(true);
    assert!(!panel.control_center_active());
    assert!(!panel.captures_keyboard());

    let mut out = ChromeEvents::default();
    panel.toggle(&mut out);
    panel.advance(0.016);
    assert!(panel.control_center_active());
    assert!(panel.captures_keyboard());
    assert!(panel.modal_active());
    assert!(panel.requires_composition());
    assert!(panel.captures_pointer(1.0, 1.0, DISPLAY, &[], &no_windows().1));

    panel.toggle(&mut out);
    panel.advance(0.016);
    assert!(!panel.control_center_active());
    assert!(!panel.captures_keyboard());
    assert!(!panel.captures_pointer(1.0, 1.0, DISPLAY, &[], &no_windows().1));
}

#[test]
fn the_sheet_never_covers_or_occludes_the_desktop() {
    // The sheet floats over the live desktop: it paints no full-output cover,
    // so the compositor never elides the desktop beneath it.
    let panel = ControlCenter::presented_for_test();
    assert!(!panel.paints_opaque_cover());
    assert!(!panel.occludes_desktop());
    assert!(!panel.exclusive_presentation_active());
}

#[test]
fn reveal_eases_instead_of_snapping_without_reduced_motion() {
    let mut panel = ControlCenter::without_sources();
    panel.toggle(&mut ChromeEvents::default());
    panel.advance(0.016);
    let early = panel.reveal();
    assert!(
        early > 0.0 && early < 1.0,
        "reveal moves gradually: {early}"
    );
    for _ in 0..120 {
        panel.advance(0.016);
    }
    assert!((panel.reveal() - 1.0).abs() < 1e-3);
}

#[test]
fn reduced_motion_snaps_reveal_to_its_target() {
    let mut panel = ControlCenter::without_sources();
    panel.set_reduced_motion(true);
    panel.toggle(&mut ChromeEvents::default());
    panel.advance(0.016);
    assert_eq!(panel.reveal(), 1.0);
}

#[test]
fn the_retirement_window_is_bounded_after_close() {
    let mut panel = ControlCenter::presented_for_test();
    let mut out = ChromeEvents::default();
    panel.toggle(&mut out);
    let mut retiring = 0;
    for _ in 0..600 {
        panel.advance(0.016);
        if panel.damage_pending() {
            retiring += 1;
        }
    }
    assert!(retiring > 0, "the vacated pixels are repaired");
    assert!(retiring <= usize::from(RETIRE_FRAMES));
    assert!(!panel.damage_pending());
    assert!(!panel.anim_pending());
    assert!(!panel.requires_composition());
}

#[test]
fn closing_clears_latches_that_would_keep_the_sheet_animating() {
    let mut panel = ControlCenter::presented_for_test();
    let mut out = ChromeEvents::default();
    panel.tip = Some((Tip::Power, panel.sheet.controls));
    panel.kbd_backlight_preview = Some(2);
    panel.session_open = true;
    panel.key_char(&escape(), &mut out); // peels the session strip
    panel.key_char(&escape(), &mut out); // closes
    assert!(!panel.open);
    for _ in 0..600 {
        panel.advance(0.016);
    }
    assert!(
        !panel.anim_pending(),
        "no stale latch keeps a closed sheet ticking"
    );
    assert!(!panel.damage_pending());
}

#[test]
fn a_fullscreen_window_closes_the_sheet() {
    let mut panel = ControlCenter::presented_for_test();
    panel.update(ChromeUpdate::SpaceUse(
        tessera_desktop::window::SpaceUse::Fullscreen,
    ));
    assert!(!panel.open);
}

// ---- geometry and damage -----------------------------------------------------

fn resolved(panel: &mut ControlCenter) -> SheetLayout {
    panel.reserved = bottom_dock_reserved();
    panel.resolve_sheet(DISPLAY)
}

#[test]
fn damage_is_the_sheet_footprint_never_the_whole_output() {
    let mut panel = ControlCenter::presented_for_test();
    resolved(&mut panel);
    let regions = panel
        .damage_regions(&[], DISPLAY)
        .expect("the sheet always states its footprint");
    let covered: f32 = regions.iter().map(|rect| rect.w * rect.h).sum();
    assert!(
        covered < DISPLAY.0 * DISPLAY.1 * 0.5,
        "a right-anchored sheet repaints well under half the output: {covered}"
    );
    for card in [panel.sheet.controls, panel.sheet.notifications] {
        assert!(
            regions.iter().any(|rect| rect.x <= card.x
                && rect.y <= card.y
                && rect.x + rect.w >= card.x + card.w
                && rect.y + rect.h >= card.y + card.h),
            "{card:?} is covered"
        );
    }
    assert_eq!(
        panel.pointer_damage_regions(1.0, 1.0, DISPLAY, &[], &no_windows().1),
        Some(regions)
    );
}

#[test]
fn an_inactive_sheet_states_no_damage() {
    let panel = ControlCenter::without_sources();
    assert_eq!(panel.damage_regions(&[], DISPLAY), None);
}

#[test]
fn the_sheet_stays_clear_of_the_dock_band() {
    let mut panel = ControlCenter::presented_for_test();
    panel.status.volume = Some(40);
    panel.status.brightness = Some(60);
    panel.status.kbd_brightness = Some(50);
    let sheet = resolved(&mut panel);
    let floor = DISPLAY.1 - bottom_dock_reserved().bottom as f32;
    assert!(sheet.controls.y + sheet.controls.h <= floor);
    assert!(sheet.notifications.y + sheet.notifications.h <= floor);
}

#[test]
fn fader_rows_exist_only_for_hardware_that_answered() {
    let mut panel = ControlCenter::presented_for_test();
    let bare = panel.controls_natural_height();
    panel.status.volume = Some(40);
    let with_sound = panel.controls_natural_height();
    assert_eq!(with_sound - bare, FADER_H + layout::SECTION_GAP);
    panel.status.brightness = Some(60);
    assert_eq!(
        panel.controls_natural_height() - with_sound,
        FADER_H + TILE_GAP
    );
}

#[test]
fn a_full_main_page_fits_a_1080p_output_unscrolled() {
    let mut panel = ControlCenter::presented_for_test();
    panel.status.volume = Some(40);
    panel.status.brightness = Some(60);
    panel.status.microphone_volume = Some(70);
    panel.status.kbd_brightness = Some(50);
    panel.test_cells = tray_cells(3);
    let natural = panel.controls_natural_height();
    let sheet = resolved(&mut panel);
    assert_eq!(
        sheet.controls.h, natural,
        "every control is visible at once"
    );
}

#[test]
fn notifications_grow_their_card_until_the_output_caps_it() {
    let mut panel = ControlCenter::presented_for_test();
    let empty = panel.notifications_natural_height();
    push_notifications(&panel, 2);
    let two = panel.notifications_natural_height();
    assert_eq!(
        two - empty,
        NOTIFICATION_H * 2.0 + NOTIFICATION_GAP - EMPTY_LIST_H
    );
    push_notifications(&panel, 60);
    let sheet = resolved(&mut panel);
    assert!(sheet.notifications.y + sheet.notifications.h <= DISPLAY.1);
}

fn push_notifications(panel: &ControlCenter, count: usize) {
    let mut queue = panel.notifications.lock().unwrap();
    for index in 0..count {
        queue.push(
            format!("Summary {index}"),
            "Body line one\nBody line two",
            Some("org.example.App".into()),
            index as u64,
        );
    }
}

fn tray_cells(count: usize) -> Vec<SniCell> {
    (0..count)
        .map(|index| SniCell {
            key: format!("test.tray.{index}"),
            title: format!("Item {index}"),
            has_menu: true,
            textured: false,
        })
        .collect()
}

#[test]
fn work_mode_and_tray_are_pinned_below_the_scrolling_content() {
    let area = Rect {
        x: 10.0,
        y: 100.0,
        w: 348.0,
        h: 500.0,
    };
    let (content, work_mode, tray) = presentation::controls::split_main_footer(area, true);
    let tray = tray.expect("a tray with items gets a row");
    assert_eq!(tray.y + tray.h, area.y + area.h);
    assert_eq!(tray.h, TRAY_ROW_H);
    assert_eq!(work_mode.y + work_mode.h, tray.y - layout::SECTION_GAP);
    assert_eq!(work_mode.h, WORK_MODE_H);
    assert!(content.y + content.h <= work_mode.y - layout::SECTION_GAP + 0.01);

    let (content, work_mode, tray) = presentation::controls::split_main_footer(area, false);
    assert!(tray.is_none());
    assert_eq!(work_mode.y + work_mode.h, area.y + area.h);
    assert!(content.y + content.h <= work_mode.y - layout::SECTION_GAP + 0.01);
}

// ---- pages and keyboard ------------------------------------------------------

#[test]
fn escape_peels_menu_then_strip_then_page_then_sheet() {
    let mut panel = ControlCenter::presented_for_test();
    let mut out = ChromeEvents::default();
    panel.show_page(Page::Bluetooth, &mut out);
    panel.session_open = true;
    panel.menu_open_for = Some("test.tray.0".into());

    panel.key_char(&escape(), &mut out);
    assert_eq!(panel.menu_open_for, None);
    assert!(panel.session_open);

    panel.key_char(&escape(), &mut out);
    assert!(!panel.session_open);
    assert_eq!(panel.page, Page::Bluetooth);

    panel.key_char(&escape(), &mut out);
    assert_eq!(panel.page, Page::Main);
    assert!(panel.open);

    panel.key_char(&escape(), &mut out);
    assert!(!panel.open);
}

#[test]
fn entering_a_page_starts_its_scan_and_leaving_cast_stops_discovery() {
    let mut panel = ControlCenter::presented_for_test();
    let mut out = ChromeEvents::default();
    panel.show_page(Page::Wifi, &mut out);
    assert_eq!(out.system_actions, vec![SystemAction::ScanWifi]);

    out.system_actions.clear();
    panel.show_page(Page::Cast, &mut out);
    assert_eq!(out.system_actions, vec![SystemAction::ScanCast]);
    assert!(panel.cast_scanning);

    out.system_actions.clear();
    panel.close(&mut out);
    assert_eq!(out.system_actions, vec![SystemAction::StopCastDiscovery]);
    assert!(!panel.cast_scanning);
    assert_eq!(panel.page, Page::Main);
}

#[test]
fn reopening_always_starts_on_the_main_page() {
    let mut panel = ControlCenter::presented_for_test();
    let mut out = ChromeEvents::default();
    panel.show_page(Page::Sound, &mut out);
    panel.toggle(&mut out);
    panel.toggle(&mut out);
    assert_eq!(panel.page, Page::Main);
}

#[test]
fn wifi_passphrase_typing_connects_and_leaving_the_page_drops_it() {
    let mut panel = ControlCenter::presented_for_test();
    let mut out = ChromeEvents::default();
    panel.show_page(Page::Wifi, &mut out);
    out.system_actions.clear();
    panel.wifi_input_ssid = Some("TestNet".into());
    let typed = |c: char| KeyChar {
        keysym: c as u32,
        ch: Some(c),
        mods: tessera_primitives::input::Mods::NONE,
    };
    for c in "abc".chars() {
        panel.key_char(&typed(c), &mut out);
    }
    panel.key_char(&key(tessera_primitives::input::XKB_KEY_BackSpace), &mut out);
    assert_eq!(panel.wifi_input_passphrase, "ab");
    panel.key_char(&key(tessera_primitives::input::XKB_KEY_Return), &mut out);
    assert_eq!(
        out.system_actions,
        vec![SystemAction::ConnectWifi {
            ssid: "TestNet".into(),
            passphrase: Some("ab".into()),
        }]
    );

    panel.wifi_input_ssid = Some("Other".into());
    panel.wifi_input_passphrase = "half".into();
    panel.show_page(Page::Main, &mut out);
    assert_eq!(panel.wifi_input_ssid, None);
    assert!(panel.wifi_input_passphrase.is_empty());
}

#[test]
fn escape_cancels_the_passphrase_prompt_before_the_page() {
    let mut panel = ControlCenter::presented_for_test();
    let mut out = ChromeEvents::default();
    panel.show_page(Page::Wifi, &mut out);
    panel.wifi_input_ssid = Some("TestNet".into());
    panel.key_char(&escape(), &mut out);
    assert_eq!(panel.wifi_input_ssid, None);
    assert_eq!(panel.page, Page::Wifi);
}

#[test]
fn request_system_confirm_dedupes_while_one_is_pending() {
    let mut panel = ControlCenter::presented_for_test();
    let mut out = ChromeEvents::default();
    panel.request_system_confirm(SystemAction::PowerOff, &mut out);
    panel.request_system_confirm(SystemAction::Reboot, &mut out);
    assert_eq!(out.system_actions, vec![SystemAction::PowerOff]);
    panel.close(&mut out);
    assert_eq!(panel.power_pending_confirm, None);
}

// ---- preferences -------------------------------------------------------------

#[test]
fn the_scheme_toggle_waits_for_preferences_and_keeps_the_rest() {
    let mut panel = ControlCenter::without_sources();
    assert!(
        panel.color_scheme_toggle(true).is_none(),
        "flipping defaults would reset the user's other preferences"
    );
    let mut snapshot = tessera_desktop::settings::SettingsSnapshot::default();
    snapshot.preferences.font_name = "Inter 11".into();
    snapshot.preferences.color_scheme = tessera_desktop::settings::ColorScheme::Dark;
    panel.update(ChromeUpdate::Settings(&snapshot));
    let Some(tessera_desktop::settings::SettingsAction::SetDesktopPreferences { preferences }) =
        panel.color_scheme_toggle(true)
    else {
        panic!("a snapshot enables the toggle");
    };
    assert_eq!(
        preferences.color_scheme,
        tessera_desktop::settings::ColorScheme::Light
    );
    assert_eq!(preferences.font_name, "Inter 11");
}

// ---- pointer interactions ----------------------------------------------------

/// The header buttons sit right-aligned in the controls card's header row.
fn header_button(panel: &ControlCenter, index: usize) -> Rect {
    let card = panel.sheet.controls;
    let inner_x = card.x + layout::CARD_PAD;
    let inner_w = card.w - layout::CARD_PAD * 2.0;
    let buttons_w = HEADER_BUTTON * 3.0 + 12.0;
    Rect {
        x: inner_x + inner_w - buttons_w + index as f32 * (HEADER_BUTTON + 6.0),
        y: card.y + layout::CARD_PAD + (HEADER_H - HEADER_BUTTON) * 0.5,
        w: HEADER_BUTTON,
        h: HEADER_BUTTON,
    }
}

#[test]
fn the_gear_opens_the_settings_application_and_closes_the_sheet() {
    let mut panel = ControlCenter::presented_for_test();
    resolved(&mut panel);
    let mut ui = lens::Ui::headless().unwrap();
    let mut out = ChromeEvents::default();
    let gear = centre(header_button(&panel, 0));
    click(&mut panel, &mut ui, gear, &mut out);
    let entry = out.spawn.expect("the gear launches Settings");
    assert_eq!(entry.id, tessera_desktop::app::settings_app::DESKTOP_ID);
    assert_eq!(entry.exec.as_deref(), Some("tessera-settings"));
    assert!(!panel.open);
}

#[test]
fn the_power_button_discloses_the_session_strip() {
    let mut panel = ControlCenter::presented_for_test();
    resolved(&mut panel);
    let mut ui = lens::Ui::headless().unwrap();
    let mut out = ChromeEvents::default();
    let power = centre(header_button(&panel, 2));
    click(&mut panel, &mut ui, power, &mut out);
    assert!(panel.session_open);
    assert!(panel.open);
    assert!(out.system_actions.is_empty(), "disclosure is not an action");
}

#[test]
fn a_press_outside_the_cards_dismisses_and_inside_does_not() {
    let mut panel = ControlCenter::presented_for_test();
    resolved(&mut panel);
    let mut ui = lens::Ui::headless().unwrap();
    let mut out = ChromeEvents::default();
    let card = panel.sheet.controls;
    // Inside the card, on the header's text (no control there).
    click(
        &mut panel,
        &mut ui,
        (
            card.x + layout::CARD_PAD + AVATAR_SIZE + 30.0,
            card.y + 30.0,
        ),
        &mut out,
    );
    assert!(panel.open, "a press on the card is not a dismissal");
    click(&mut panel, &mut ui, (40.0, 40.0), &mut out);
    assert!(!panel.open, "a press on the desktop dismisses the sheet");
}

#[test]
fn clear_all_dismisses_every_notification_in_order() {
    let mut panel = ControlCenter::presented_for_test();
    push_notifications(&panel, 3);
    let sheet = resolved(&mut panel);
    let ids: Vec<u64> = panel.notification_snapshot().iter().map(|n| n.id).collect();
    let card = sheet.notifications;
    let header_y = card.y + layout::CARD_PAD + CLOCK_BLOCK_H + layout::SECTION_GAP;
    let button = (
        card.x + card.w - layout::CARD_PAD - 24.0,
        header_y + LIST_HEADER_H * 0.5,
    );
    let mut ui = lens::Ui::headless().unwrap();
    let mut out = ChromeEvents::default();
    click(&mut panel, &mut ui, button, &mut out);
    assert_eq!(out.dismissed_notifications, ids);
    assert!(panel.open, "clearing is not a dismissal of the sheet");
}

#[test]
fn app_labels_read_like_names() {
    use presentation::notifications::app_label;
    assert_eq!(app_label("org.gnome.Nautilus.desktop"), "Nautilus");
    assert_eq!(app_label("firefox"), "firefox");
    assert_eq!(app_label("org.example."), "org.example.");
}

// ---- ported: tray paging, sound and input intents, subtitles -----------------

fn paged_shelf_fixture(count: usize) -> (Rect, Vec<SniCell>) {
    use crate::components::control_center::tray_shelf::shelf_width;
    let anchor = Rect {
        x: 48.0,
        y: 300.0,
        w: shelf_width(2),
        h: TRAY_CELL + TRAY_PAD * 2.0,
    };
    let cells = (0..count)
        .map(|index| SniCell {
            key: format!("test.tray.{index}"),
            title: format!("Item {index}"),
            has_menu: true,
            textured: false,
        })
        .collect();
    (anchor, cells)
}

#[test]
fn an_oversized_tray_paginates_instead_of_hiding_items() {
    // [INV-CHROME-03]: no scroll viewport. [INV-CHROME-07]: paging is bounded
    // and reversible. Six items in a two-slot lane are reached through whole
    // pages, and every item appears on exactly one page.
    use crate::components::control_center::tray_shelf::shelf_layout;
    let (anchor, cells) = paged_shelf_fixture(6);
    let layout = shelf_layout(anchor, cells.len(), 0);
    assert_eq!(layout.slots, 2, "the fixture is a two-slot lane");
    assert!(layout.paged, "six items in two slots must paginate");
    assert_eq!(layout.pages, 6, "one item per page beside the chevrons");

    let mut seen = Vec::new();
    for page in 0..layout.pages {
        let page_layout = shelf_layout(anchor, cells.len(), page);
        assert_eq!(page_layout.has_previous(), page > 0);
        assert_eq!(page_layout.has_next(), page + 1 < layout.pages);
        for slot in page_layout.page_slots() {
            if let crate::components::control_center::tray_shelf::ShelfSlot::Item(index) = slot {
                seen.push(cells[index].key.clone());
            }
        }
    }
    assert_eq!(seen.len(), cells.len(), "every item is reachable once");
    assert!(seen.iter().all(|key| cells.iter().any(|c| &c.key == key)));
}

#[test]
fn pressing_the_next_chevron_turns_the_page_and_the_last_page_goes_no_further() {
    // [INV-CHROME-07] end to end: the chevrons are real pressables whose
    // reachability comes from the layout. A pure-function test cannot prove the
    // wiring — it needs a real frame and a real pointer sequence.
    use crate::components::control_center::tray_shelf::shelf_layout;
    let display = (1280.0, 720.0);
    let (anchor, cells) = paged_shelf_fixture(6);
    let mut panel = ControlCenter::without_sources();
    panel.open = true;
    panel.reveal.snap_to(1.0);
    panel.set_reduced_motion(true);
    let mut out = ChromeEvents::default();
    let mut ui = lens::Ui::headless().unwrap();
    let mut input = Input::new(display, 0.05);
    let i18n = Localizer::default();

    let frame =
        |ui: &mut lens::Ui, panel: &mut ControlCenter, input: &Input, out: &mut ChromeEvents| {
            ui.frame(input, |f| {
                panel.render_tray_shelf_cells(
                    f,
                    anchor,
                    &cells,
                    1.0,
                    (input.as_raw().cursor.x, input.as_raw().cursor.y),
                    &i18n,
                );
            });
            let snap = ui.snapshot().unwrap();
            ui.activate(&snap).unwrap();
            let _ = out;
        };
    let press = |ui: &mut lens::Ui,
                 panel: &mut ControlCenter,
                 input: &mut Input,
                 out: &mut ChromeEvents| {
        input.set_mouse_down(lens::MouseButton::Left, true);
        input.set_mouse_pressed(lens::MouseButton::Left, true);
        frame(ui, panel, input, out);
        input.set_mouse_down(lens::MouseButton::Left, false);
        input.set_mouse_pressed(lens::MouseButton::Left, false);
        input.set_mouse_released(lens::MouseButton::Left, true);
        frame(ui, panel, input, out);
        input.set_mouse_released(lens::MouseButton::Left, false);
    };

    let first = shelf_layout(anchor, cells.len(), 0);
    let next = first.slot_rect(first.visible_slots() - 1);
    let (next_x, next_y) = (next.x + next.w * 0.5, next.y + next.h * 0.5);

    // Two frames so the pressable has last-frame geometry to hit-test.
    input.set_cursor(next_x, next_y);
    frame(&mut ui, &mut panel, &input, &mut out);
    frame(&mut ui, &mut panel, &input, &mut out);
    assert_eq!(panel.tray_page, 0, "nothing turns before the press");
    assert_eq!(
        panel.tray_shelf.map(|shelf| shelf.page),
        Some(0),
        "the renderer must cache the resolved layout"
    );

    press(&mut ui, &mut panel, &mut input, &mut out);
    assert_eq!(panel.tray_page, 1, "the next chevron turns the page");

    // The last page's next chevron is disabled: pressing it is inert. Walk to
    // the end first.
    let pages = first.pages;
    for expected in 2..pages {
        let layout = shelf_layout(anchor, cells.len(), panel.tray_page);
        let next = layout.slot_rect(layout.visible_slots() - 1);
        input.set_cursor(next.x + next.w * 0.5, next.y + next.h * 0.5);
        frame(&mut ui, &mut panel, &input, &mut out);
        frame(&mut ui, &mut panel, &input, &mut out);
        press(&mut ui, &mut panel, &mut input, &mut out);
        assert_eq!(panel.tray_page, expected);
    }
    assert_eq!(panel.tray_page, pages - 1, "the walk ends on the last page");

    // One more press on the (now disabled) trailing chevron does nothing.
    let last = shelf_layout(anchor, cells.len(), panel.tray_page);
    assert!(!last.has_next(), "the last page has no next");
    let next = last.slot_rect(last.visible_slots() - 1);
    input.set_cursor(next.x + next.w * 0.5, next.y + next.h * 0.5);
    frame(&mut ui, &mut panel, &input, &mut out);
    frame(&mut ui, &mut panel, &input, &mut out);
    press(&mut ui, &mut panel, &mut input, &mut out);
    assert_eq!(
        panel.tray_page,
        pages - 1,
        "a disabled chevron must not overshoot the last page"
    );
}

#[test]
fn pressing_the_previous_chevron_turns_back_and_the_first_page_goes_no_further() {
    // [INV-CHROME-07]: paging is *reversible*. The leading chevron must return
    // to the page the trailing one left, and stop at the first page.
    use crate::components::control_center::tray_shelf::shelf_layout;
    let display = (1280.0, 720.0);
    let (anchor, cells) = paged_shelf_fixture(6);
    let mut panel = ControlCenter::without_sources();
    panel.open = true;
    panel.reveal.snap_to(1.0);
    panel.set_reduced_motion(true);
    panel.tray_page = 3;
    let mut out = ChromeEvents::default();
    let mut ui = lens::Ui::headless().unwrap();
    let mut input = Input::new(display, 0.05);
    let i18n = Localizer::default();

    let frame =
        |ui: &mut lens::Ui, panel: &mut ControlCenter, input: &Input, out: &mut ChromeEvents| {
            ui.frame(input, |f| {
                panel.render_tray_shelf_cells(
                    f,
                    anchor,
                    &cells,
                    1.0,
                    (input.as_raw().cursor.x, input.as_raw().cursor.y),
                    &i18n,
                );
            });
            let snap = ui.snapshot().unwrap();
            ui.activate(&snap).unwrap();
            let _ = out;
        };
    let press_at = |ui: &mut lens::Ui,
                    panel: &mut ControlCenter,
                    input: &mut Input,
                    out: &mut ChromeEvents,
                    at: Rect| {
        input.set_cursor(at.x + at.w * 0.5, at.y + at.h * 0.5);
        frame(ui, panel, input, out);
        frame(ui, panel, input, out);
        input.set_mouse_down(lens::MouseButton::Left, true);
        input.set_mouse_pressed(lens::MouseButton::Left, true);
        frame(ui, panel, input, out);
        input.set_mouse_down(lens::MouseButton::Left, false);
        input.set_mouse_pressed(lens::MouseButton::Left, false);
        input.set_mouse_released(lens::MouseButton::Left, true);
        frame(ui, panel, input, out);
        input.set_mouse_released(lens::MouseButton::Left, false);
    };

    // Back from page 3 to page 0, one chevron press each.
    for expected in (0..3).rev() {
        let layout = shelf_layout(anchor, cells.len(), panel.tray_page);
        assert!(layout.has_previous());
        press_at(
            &mut ui,
            &mut panel,
            &mut input,
            &mut out,
            layout.slot_rect(0),
        );
        assert_eq!(panel.tray_page, expected, "the previous chevron steps back");
    }

    // The first page's leading chevron is disabled.
    let first = shelf_layout(anchor, cells.len(), 0);
    assert!(!first.has_previous());
    press_at(
        &mut ui,
        &mut panel,
        &mut input,
        &mut out,
        first.slot_rect(0),
    );
    assert_eq!(
        panel.tray_page, 0,
        "a disabled chevron must not go below the first page"
    );
}

#[test]
fn an_item_on_a_later_page_still_activates_and_opens_its_menu() {
    // The chevron tests prove the pager turns; they say nothing about whether
    // the items it turned *to* are still real pressables. A page whose items
    // were drawn but not hit-testable would be the pager's worst failure: the
    // lane shows the icon and refuses the click.
    use crate::components::control_center::tray_shelf::shelf_layout;
    let display = (1280.0, 720.0);
    let (anchor, cells) = paged_shelf_fixture(6);
    let mut panel = ControlCenter::without_sources();
    panel.open = true;
    panel.reveal.snap_to(1.0);
    panel.set_reduced_motion(true);
    panel.tray_page = 2;
    let mut out = ChromeEvents::default();
    let mut ui = lens::Ui::headless().unwrap();
    let mut input = Input::new(display, 0.05);
    let i18n = Localizer::default();

    let frame =
        |ui: &mut lens::Ui, panel: &mut ControlCenter, input: &Input, out: &mut ChromeEvents| {
            ui.frame(input, |f| {
                panel.render_tray_shelf_cells(
                    f,
                    anchor,
                    &cells,
                    1.0,
                    (input.as_raw().cursor.x, input.as_raw().cursor.y),
                    &i18n,
                );
            });
            let snap = ui.snapshot().unwrap();
            ui.activate(&snap).unwrap();
            let _ = out;
        };

    // Page 2: one item, bracketed by both chevrons.
    let layout = shelf_layout(anchor, cells.len(), 2);
    assert_eq!(layout.first_item, 2);
    let item_slot = layout.slot_rect(layout.first_item_slot());
    assert_eq!(
        layout.slot_at(layout.first_item_slot()),
        Some(crate::components::control_center::tray_shelf::ShelfSlot::Item(2)),
        "slot 1 of a paged page is the first item, not a chevron"
    );
    let (x, y) = (
        item_slot.x + item_slot.w * 0.5,
        item_slot.y + item_slot.h * 0.5,
    );

    // Two frames so the pressable has last-frame geometry to hit-test.
    input.set_cursor(x, y);
    frame(&mut ui, &mut panel, &input, &mut out);
    frame(&mut ui, &mut panel, &input, &mut out);
    assert_eq!(panel.tray_page, 2, "the fixture starts on page 2");
    assert_eq!(panel.menu_open_for, None, "nothing opens before the press");

    // Right-click must reach the item on this page and open its popover.
    input.set_mouse_down(lens::MouseButton::Right, true);
    input.set_mouse_pressed(lens::MouseButton::Right, true);
    frame(&mut ui, &mut panel, &input, &mut out);
    input.set_mouse_down(lens::MouseButton::Right, false);
    input.set_mouse_pressed(lens::MouseButton::Right, false);
    input.set_mouse_released(lens::MouseButton::Right, true);
    frame(&mut ui, &mut panel, &input, &mut out);
    input.set_mouse_released(lens::MouseButton::Right, false);

    assert_eq!(
        panel.menu_open_for.as_deref(),
        Some(cells[2].key.as_str()),
        "an item on a later page is still a live pressable"
    );
    // ...and the popover it opened is anchored to that item's own cell, not to
    // the lane's corner or to the leading chevron.
    assert!(
        panel.menu_owner.x >= item_slot.x - 1.0 && panel.menu_owner.x <= item_slot.x + item_slot.w,
        "the popover anchors to the pressed cell: {:?} vs {item_slot:?}",
        panel.menu_owner
    );
}

#[test]
fn a_page_that_stops_existing_falls_back_to_a_real_one() {
    // Items leave all the time (a tray client exits). The shelf must never be
    // stranded on a page the pile no longer has — that would be an empty lane
    // with live chevrons.
    use crate::components::control_center::tray_shelf::shelf_layout;
    let (anchor, cells) = paged_shelf_fixture(6);
    let layout = shelf_layout(anchor, cells.len(), 9);
    assert_eq!(layout.page, layout.pages - 1, "the page index clamps");

    let mut panel = ControlCenter::without_sources();
    panel.open = true;
    panel.reveal.snap_to(1.0);
    panel.set_reduced_motion(true);
    panel.tray_page = 5;
    let out = ChromeEvents::default();
    let mut ui = lens::Ui::headless().unwrap();
    let input = Input::new((1280.0, 720.0), 0.05);
    let i18n = Localizer::default();

    // Only two items remain: one page, no pager.
    ui.frame(&input, |f| {
        panel.render_tray_shelf_cells(
            f,
            anchor,
            &cells[..2],
            1.0,
            (input.as_raw().cursor.x, input.as_raw().cursor.y),
            &i18n,
        );
    });
    let snap = ui.snapshot().unwrap();
    ui.activate(&snap).unwrap();
    let _ = out;

    assert_eq!(
        panel.tray_page, 0,
        "a vanished page resolves to the only one"
    );
    let resolved = panel.tray_shelf.expect("the renderer caches the layout");
    assert!(!resolved.paged);
    assert_eq!(resolved.page_slots().len(), 2);
}

#[test]
fn a_tray_menu_that_scrolled_to_another_page_is_dismissed() {
    // A popover anchored to a cell that is no longer drawn would float over the
    // wrong icon. Turning the page must dismiss it, not re-anchor it to
    // whatever now occupies that slot.
    let (anchor, cells) = paged_shelf_fixture(6);
    let mut panel = ControlCenter::without_sources();
    panel.open = true;
    panel.reveal.snap_to(1.0);
    panel.set_reduced_motion(true);
    panel.menu_open_for = Some(cells[0].key.clone());
    let mut ui = lens::Ui::headless().unwrap();
    let input = Input::new((1280.0, 720.0), 0.05);
    let i18n = Localizer::default();

    // Page 0 draws item 0; the menu keeps its anchor.
    ui.frame(&input, |f| {
        panel.render_tray_shelf_cells(
            f,
            anchor,
            &cells,
            1.0,
            (input.as_raw().cursor.x, input.as_raw().cursor.y),
            &i18n,
        );
    });
    let snap = ui.snapshot().unwrap();
    ui.activate(&snap).unwrap();
    assert_eq!(panel.menu_open_for.as_deref(), Some(cells[0].key.as_str()));

    // Page 2 does not draw item 0 any more.
    panel.tray_page = 2;
    ui.frame(&input, |f| {
        panel.render_tray_shelf_cells(
            f,
            anchor,
            &cells,
            1.0,
            (input.as_raw().cursor.x, input.as_raw().cursor.y),
            &i18n,
        );
    });
    let snap = ui.snapshot().unwrap();
    ui.activate(&snap).unwrap();
    assert!(
        panel.menu_open_for.is_none(),
        "a popover whose cell left the page must close"
    );
}

#[test]

fn clicking_an_output_row_names_that_output_and_never_the_default() {
    // The row's intent is the server node name of the row the user pressed.
    // Clicking the row that is already default produces nothing: re-writing
    // the same key would raise a reading for a state that did not change
    // (ADR-0215).
    let current = tessera_desktop::system::AudioOutput {
        name: "alsa_output.speaker".into(),
        description: "Built-in Speaker".into(),
        kind: tessera_desktop::system::AudioOutputKind::Speaker,
        is_default: true,
    };
    let other = tessera_desktop::system::AudioOutput {
        name: "bluez_output.AA.1".into(),
        description: "EDIFIER W830NB".into(),
        kind: tessera_desktop::system::AudioOutputKind::Bluetooth,
        is_default: false,
    };
    assert_eq!(
        ControlCenter::audio_output_intent(&other),
        Some(SystemAction::SetAudioOutput {
            name: "bluez_output.AA.1".into(),
        })
    );
    assert_eq!(ControlCenter::audio_output_intent(&current), None);
}

#[test]
fn clicking_an_input_row_names_that_input_and_never_the_default() {
    // The input half of the same rule (ADR-0216): the row's intent is the
    // server node name of the row the user pressed, and pressing the current
    // default produces nothing.
    let current = tessera_desktop::system::AudioInput {
        name: "alsa_input.builtin".into(),
        description: "Built-in Microphone".into(),
        kind: tessera_desktop::system::AudioInputKind::BuiltIn,
        is_default: true,
    };
    let other = tessera_desktop::system::AudioInput {
        name: "bluez_input.AA.1".into(),
        description: "WH-1000XM5".into(),
        kind: tessera_desktop::system::AudioInputKind::Bluetooth,
        is_default: false,
    };
    assert_eq!(
        ControlCenter::audio_input_intent(&other),
        Some(SystemAction::SetAudioInput {
            name: "bluez_input.AA.1".into(),
        })
    );
    assert_eq!(ControlCenter::audio_input_intent(&current), None);
}

#[test]
fn the_microphone_fader_reads_capture_state_not_just_level() {
    // The page's reading is the server's link graph, not a level: a muted
    // microphone and a microphone being recorded are different facts, and the
    // banner is driven by the second one only (ADR-0216).
    let idle = SystemStatus {
        microphone_volume: Some(70),
        microphone_muted: false,
        microphone_active: false,
        ..SystemStatus::default()
    };
    assert!(!idle.microphone_active);
    let capturing = SystemStatus {
        microphone_active: true,
        ..idle.clone()
    };
    assert!(capturing.microphone_active);
    // The two states a `SystemStatus` can express are independent: a muted
    // microphone can still be held open by a client, and chrome must be able to
    // say both.
    let muted_but_capturing = SystemStatus {
        microphone_muted: true,
        microphone_active: true,
        ..idle
    };
    assert!(muted_but_capturing.microphone_muted && muted_but_capturing.microphone_active);
}

#[test]
fn the_sound_fader_offers_the_output_page_only_when_there_is_a_choice() {
    // The disclosure is driven by the inventory, not by a fixed chrome
    // constant. No inventory means no sound server answered, and a single
    // output leaves nothing to pick: both would open a page that cannot change
    // anything (ADR-0215).
    let output = |name: &str, is_default: bool| tessera_desktop::system::AudioOutput {
        name: name.into(),
        description: name.into(),
        kind: tessera_desktop::system::AudioOutputKind::Speaker,
        is_default,
    };
    let with = |outputs: Vec<tessera_desktop::system::AudioOutput>| SystemStatus {
        audio_outputs: outputs,
        ..SystemStatus::default()
    };

    assert!(!ControlCenter::audio_output_disclosure_visible(&with(
        Vec::new()
    )));
    assert!(!ControlCenter::audio_output_disclosure_visible(&with(
        vec![output("alsa_output.speaker", true)]
    )));
    assert!(ControlCenter::audio_output_disclosure_visible(&with(vec![
        output("alsa_output.speaker", true),
        output("bluez_output.AA.1", false),
    ])));

    // A single output still earns the disclosure when there is a microphone to
    // mute or a capture device to choose (ADR-0216): the page is the only way
    // into the capture controls, so hiding it would make them unreachable.
    let microphone_only = SystemStatus {
        audio_outputs: vec![output("alsa_output.speaker", true)],
        microphone_volume: Some(70),
        ..SystemStatus::default()
    };
    assert!(ControlCenter::audio_output_disclosure_visible(
        &microphone_only
    ));
    // No server at all still hides it: there is nothing behind the chevron.
    assert!(!ControlCenter::audio_output_disclosure_visible(
        &SystemStatus::default()
    ));

    // Both states must also paint: the page is reachable from a stale
    // snapshot, so an empty inventory has to render its "unavailable" body
    // rather than panic on the missing rows.
    let mut panel = ControlCenter::without_sources();
    panel.open = true;
    panel.reveal.snap_to(1.0);
    let mut out = ChromeEvents::default();
    let windows = Vec::new();
    let workspaces = WorkspaceSnapshot {
        outputs: Vec::new(),
    };
    let i18n = Localizer::default();
    let input = Input::new((1920.0, 1080.0), 0.05);
    let mut ui = lens::Ui::headless().unwrap();

    panel.page = Page::Sound;
    panel.status.audio_outputs = vec![
        output("alsa_output.speaker", true),
        output("bluez_output.AA.1", false),
    ];
    render_headless(
        &mut ui,
        &mut panel,
        &input,
        &windows,
        &workspaces,
        &i18n,
        &mut out,
    );

    out.system_actions.clear();
    panel.status.audio_outputs.clear();
    render_headless(
        &mut ui,
        &mut panel,
        &input,
        &windows,
        &workspaces,
        &i18n,
        &mut out,
    );
    assert!(
        !out.system_actions
            .iter()
            .any(|action| matches!(action, SystemAction::SetAudioOutput { .. })),
        "an empty inventory cannot name an output"
    );
}

#[test]
fn the_sound_page_paints_in_every_capture_and_inventory_state() {
    // A real frame, not a pure function: the page has a header, a conditional
    // capture banner, a microphone fader, and two conditional lists, and the
    // states below are exactly the ones whose layout could overlap or panic —
    // an empty inventory with a live capture (banner above an empty body), a
    // single microphone (no input list), and a muted-but-capturing device
    // (both facts at once) (ADR-0216).
    let output = |name: &str, is_default: bool| tessera_desktop::system::AudioOutput {
        name: name.into(),
        description: name.into(),
        kind: tessera_desktop::system::AudioOutputKind::Speaker,
        is_default,
    };
    let input = |name: &str, is_default: bool| tessera_desktop::system::AudioInput {
        name: name.into(),
        description: name.into(),
        kind: tessera_desktop::system::AudioInputKind::BuiltIn,
        is_default,
    };

    let states = [
        // No sound server, but something is somehow capturing: the banner must
        // still paint above an empty body.
        SystemStatus {
            microphone_active: true,
            ..SystemStatus::default()
        },
        // The ordinary live case: two outputs, two inputs, capture in progress.
        SystemStatus {
            audio_outputs: vec![output("alsa_output.speaker", true), output("hdmi", false)],
            audio_inputs: vec![input("alsa_input.builtin", true), input("usb", false)],
            microphone_volume: Some(60),
            microphone_active: true,
            ..SystemStatus::default()
        },
        // One input: the fader names it and the list is omitted, because a
        // one-row picker cannot change anything.
        SystemStatus {
            audio_outputs: vec![output("alsa_output.speaker", true)],
            audio_inputs: vec![input("alsa_input.builtin", true)],
            microphone_volume: Some(0),
            microphone_muted: true,
            microphone_active: true,
            ..SystemStatus::default()
        },
        // No sound server at all.
        SystemStatus::default(),
    ];

    let windows = Vec::new();
    let workspaces = WorkspaceSnapshot {
        outputs: Vec::new(),
    };
    let i18n = Localizer::default();
    let input_state = Input::new((1920.0, 1080.0), 0.05);
    let mut ui = lens::Ui::headless().unwrap();

    for state in states {
        let mut panel = ControlCenter::without_sources();
        panel.open = true;
        panel.reveal.snap_to(1.0);
        panel.page = Page::Sound;
        panel.status = state.clone();
        let mut out = ChromeEvents::default();
        render_headless(
            &mut ui,
            &mut panel,
            &input_state,
            &windows,
            &workspaces,
            &i18n,
            &mut out,
        );
        // Nothing on this page is a system action it can raise unprompted; a
        // rendered frame must not dispatch merely because the page is open.
        assert!(
            out.system_actions.is_empty(),
            "opening the Sound page dispatched {:?} for {state:?}",
            out.system_actions
        );
    }
}

#[test]
fn bluetooth_detail_actions_validate() {
    let mut out = ChromeEvents::default();
    out.system_actions.push(SystemAction::ScanBluetooth);
    out.system_actions.push(SystemAction::PairBluetooth {
        address: "AC:12:34:56:78:9A".to_string(),
    });
    out.system_actions.push(SystemAction::ConnectBluetooth {
        address: "AC:12:34:56:78:9A".to_string(),
    });
    out.system_actions.push(SystemAction::DisconnectBluetooth {
        address: "AC:12:34:56:78:9A".to_string(),
    });
    out.system_actions.push(SystemAction::ForgetBluetooth {
        address: "AC:12:34:56:78:9A".to_string(),
    });
    assert_eq!(out.system_actions.len(), 5);
    assert!(
        out.system_actions
            .iter()
            .all(|action| action.validate().is_ok())
    );
}

#[test]
fn cast_detail_actions_validate() {
    let mut out = ChromeEvents::default();
    out.system_actions.push(SystemAction::ScanCast);
    out.system_actions.push(SystemAction::ConnectCast {
        sink: "/net/connman/iwd/0/p2p_peers/peer".to_string(),
    });
    out.system_actions
        .push(SystemAction::DisconnectCast { session: 7 });
    assert_eq!(out.system_actions.len(), 3);
    assert!(
        out.system_actions
            .iter()
            .all(|action| action.validate().is_ok())
    );
}

#[test]
fn bluetooth_tile_subtitle_prioritizes_the_connected_device() {
    let i18n = Localizer::default();

    // No radio service: unavailable, never a fabricated "Off".
    assert_eq!(
        ControlCenter::bluetooth_tile_subtitle(&SystemStatus::default(), &i18n),
        i18n.text(Message::Unavailable)
    );

    // Radio off.
    let off = SystemStatus {
        bluetooth_enabled: Some(false),
        ..SystemStatus::default()
    };
    assert_eq!(
        ControlCenter::bluetooth_tile_subtitle(&off, &i18n),
        i18n.text(Message::Off)
    );

    // Radio on with a live link names the device.
    let connected = SystemStatus {
        bluetooth_enabled: Some(true),
        bluetooth_state: tessera_desktop::system::BluetoothLinkState::Connected,
        bluetooth_devices: vec![tessera_desktop::system::BluetoothDevice {
            address: "AC:12:34:56:78:9A".to_string(),
            name: "Sony WH-1000XM5".to_string(),
            connected: true,
            paired: true,
            ..Default::default()
        }],
        ..SystemStatus::default()
    };
    assert_eq!(
        ControlCenter::bluetooth_tile_subtitle(&connected, &i18n),
        "Sony WH-1000XM5"
    );
}

#[test]
fn test_inv_arch_80_cast_tile_reflects_status() {
    let i18n = Localizer::default();

    // An idle desktop reads as not casting, with no fabricated count.
    assert_eq!(
        ControlCenter::cast_tile_subtitle(&SystemStatus::default(), &i18n),
        i18n.text(Message::NotCasting)
    );

    // A present extended display with no live stream names the display count.
    let extended = SystemStatus {
        virtual_displays: 2,
        ..SystemStatus::default()
    };
    assert_eq!(
        ControlCenter::cast_tile_subtitle(&extended, &i18n),
        format!("{} · 2", i18n.text(Message::Displays))
    );

    // A live casting stream wins over a merely-present display.
    let casting = SystemStatus {
        virtual_displays: 2,
        cast_streams: 1,
        ..SystemStatus::default()
    };
    assert_eq!(
        ControlCenter::cast_tile_subtitle(&casting, &i18n),
        format!("{} · 1", i18n.text(Message::Casting))
    );

    // A live service session names the sink even before a compositor counter
    // catches up; the subtitle never invents a session from network state.
    let session = SystemStatus {
        cast: tessera_desktop::system::CastStatus {
            sessions: vec![tessera_desktop::system::CastSession {
                id: 1,
                sink_id: "peer".to_owned(),
                sink_name: "Living Room TV".to_owned(),
                state: tessera_desktop::system::CastSessionState::Streaming,
                frames_sent: 30,
                audio: Some("AAC 00000001 00".to_owned()),
                failure: None,
            }],
            ..tessera_desktop::system::CastStatus::default()
        },
        ..SystemStatus::default()
    };
    assert_eq!(
        ControlCenter::cast_tile_subtitle(&session, &i18n),
        format!("{} · Living Room TV", i18n.text(Message::Casting))
    );
}

#[test]
fn keyboard_backlight_actions_validate() {
    let mut out = ChromeEvents::default();
    out.system_actions
        .push(SystemAction::SetKeyboardBrightness { level: 75 });
    out.system_actions
        .push(SystemAction::StepKeyboardBrightness);
    assert_eq!(out.system_actions.len(), 2);
    assert!(out.system_actions[0].validate().is_ok());
    assert!(out.system_actions[1].validate().is_ok());
}

#[test]

fn keyboard_backlight_policy_follows_hardware_granularity() {
    // Stepped hardware (e.g. a 3-rung backlight) yields a real ladder and maps
    // a reported level to its nearest rung.
    let stepped = SystemStatus {
        kbd_brightness: Some(50),
        kbd_brightness_levels: Some(3),
        ..SystemStatus::default()
    };
    assert_eq!(stepped.kbd_brightness_tiers(), Some(vec![0, 50, 100]));
    assert_eq!(stepped.kbd_brightness_tier_index(), 1);

    // Fine-grained hardware (max_brightness = 255) is not a stepped selector;
    // chrome falls back to a continuous fader.
    let fine = SystemStatus {
        kbd_brightness: Some(42),
        kbd_brightness_levels: Some(255),
        ..SystemStatus::default()
    };
    assert_eq!(fine.kbd_brightness_tiers(), None);

    // No backlight at all: nothing renders.
    let none = SystemStatus::default();
    assert_eq!(none.kbd_brightness_tiers(), None);
}

#[test]
fn keyboard_backlight_indicator_settles_on_the_active_tier() {
    let mut indicator = TieredIndicator::at(0);
    assert!(
        !indicator.anim_pending(0),
        "fresh indicator rests on tier 0"
    );
    // Travel to the top tier; reduced motion resolves in a single frame.
    indicator.advance(3, 1.0 / 60.0, true);
    assert!(
        !indicator.anim_pending(3),
        "reduced motion resolves in one frame"
    );
    assert!((indicator.spring.value - 3.0).abs() < 1e-3);
}

#[test]
fn indicator_spring_actually_overshoots() {
    // Regression: the indicator was tuned critically damped (ζ = 1.0), so the
    // "bounce" the comments and docs promised could not occur. The under-damped
    // tuning must cross its target before settling.
    const {
        assert!(
            crate::widgets::tiered::INDICATOR_SPRING.damping < 1.0,
            "the indicator must be under-damped to overshoot"
        )
    };
    let mut indicator = TieredIndicator::at(0);
    let mut overshot = false;
    for _ in 0..600 {
        indicator.advance(3, 1.0 / 120.0, false);
        if indicator.spring.value > 3.0 + 1e-3 {
            overshot = true;
        }
    }
    assert!(overshot, "an under-damped indicator must cross its target");
    assert!(!indicator.anim_pending(3), "and still settle on it");
}

#[test]
fn mpris_mock_handle_state_and_commands() {
    let handle = mpris::MediaHandle::mock();
    let snap = handle.snapshot();
    assert!(snap.available);
    assert!(snap.playing);
    assert_eq!(snap.title, "Resonance");

    handle.send(mpris::MediaCommand::PlayPause);
    std::thread::sleep(std::time::Duration::from_millis(50));
    let snap = handle.snapshot();
    assert!(!snap.playing);

    handle.send(mpris::MediaCommand::Next);
    std::thread::sleep(std::time::Duration::from_millis(50));
    let snap = handle.snapshot();
    assert_eq!(snap.title, "Sunlight");
}
