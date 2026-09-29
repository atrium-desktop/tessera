use super::*;

use super::presentation::canvas_alpha;

fn escape() -> KeyChar {
    KeyChar {
        keysym: tessera_primitives::input::XKB_KEY_Escape,
        ch: None,
        mods: tessera_primitives::input::Mods::NONE,
    }
}

fn fullscreen_window() -> Window {
    let mut window = Window::new(tessera_desktop::window::WindowId(7));
    window.state.fullscreen = true;
    window
}

#[test]
fn toggle_opens_and_closes_the_panel() {
    let mut panel = CommandPanel::without_sources();
    panel.set_reduced_motion(true);
    assert!(!panel.command_panel_active());
    assert!(!panel.captures_keyboard());

    let mut out = ChromeEvents::default();
    panel.toggle_command_panel(&mut out);
    assert!(panel.open);
    panel.advance(0.016);
    assert!(panel.command_panel_active());
    assert!(panel.captures_keyboard());
    assert!(panel.modal_active());
    assert!(panel.exclusive_presentation_active());
    assert!(panel.requires_composition());
    // The settle drain keeps the ring alive for FLUX_MAX_FRAMES_IN_FLIGHT
    // frames even with reduced motion, so the opaque canvas reaches every slot.
    assert!(panel.anim_pending());

    // The panel paints its own opaque full-screen canvas; it declares no
    // backdrop capture, blur, or frost (ADR-0176).
    assert_eq!(panel.backdrop_blur_sigma(), 0.0);
    assert!(
        panel
            .backdrop_regions(
                (1920.0, 1080.0),
                &[],
                &WorkspaceSnapshot { outputs: Vec::new() },
            )
            .is_empty()
    );

    panel.toggle_command_panel(&mut out);
    assert!(!panel.open);
    for _ in 0..(SETTLED_DRAIN_FRAMES + 1) {
        panel.advance(0.016);
    }
    assert!(!panel.command_panel_active());
    assert!(!panel.exclusive_presentation_active());
    assert_eq!(panel.backdrop_blur_sigma(), 0.0);
}

#[test]
fn reveal_eases_instead_of_snapping_without_reduced_motion() {
    let mut panel = CommandPanel::without_sources();
    let mut out = ChromeEvents::default();
    panel.toggle_command_panel(&mut out);
    panel.advance(0.016);
    let early = panel.reveal();
    assert!(early > 0.0 && early < 1.0);
    assert!(panel.anim_pending());
    for _ in 0..240 {
        panel.advance(0.016);
    }
    assert_eq!(panel.reveal(), 1.0);
    assert!(!panel.anim_pending());
}

#[test]
fn the_settle_drain_keeps_the_panel_active_for_the_ring_depth() {
    // The opaque full-screen canvas must overwrite the pre-open desktop in
    // every swapchain ring slot; on the frame the reveal settles the panel
    // primes FLUX_MAX_FRAMES_IN_FLIGHT (3) drain frames of full-output damage
    // before releasing the chrome layer (ADR-0171 `[INV-ARCH-03]`).
    let mut panel = CommandPanel::without_sources();
    let mut out = ChromeEvents::default();
    panel.toggle_command_panel(&mut out);
    panel.reveal.snap_to(1.0);

    // Close: the reveal travels to 0, then the settle edge primes the drain.
    panel.toggle_command_panel(&mut out);
    assert!(!panel.open);
    let mut drain_primed = false;
    for _ in 0..240 {
        panel.advance(0.016);
        if panel.reveal() == 0.0 && !panel.reveal_animating() && panel.settled_drain_frames > 0 {
            drain_primed = true;
            break;
        }
    }
    assert!(drain_primed, "the settle edge primes the ring drain");
    assert!(panel.active(), "the drain keeps the chrome layer");
    assert!(panel.anim_pending(), "the drain keeps the frame loop alive");
    assert!(
        panel.damage_region(&[], (1920.0, 1080.0)).is_none(),
        "the drain forces a full-output repaint"
    );

    // Exactly FLUX_MAX_FRAMES_IN_FLIGHT further frames drain the ring, then the
    // panel releases the chrome layer.
    for _ in 0..SETTLED_DRAIN_FRAMES {
        assert!(panel.active());
        panel.advance(0.016);
    }
    assert!(!panel.anim_pending());
    assert!(!panel.active(), "the chrome layer is released after the drain");
}

#[test]
fn reduced_motion_snaps_reveal_to_its_target() {
    let mut panel = CommandPanel::without_sources();
    let mut out = ChromeEvents::default();
    panel.toggle_command_panel(&mut out);
    panel.set_reduced_motion(true);
    assert_eq!(panel.reveal(), 1.0);
    panel.set_reduced_motion(false);
    panel.toggle_command_panel(&mut out);
    panel.set_reduced_motion(true);
    assert_eq!(panel.reveal(), 0.0);
}

#[test]
fn a_playing_avatar_keeps_the_panel_frame_loop_alive_after_reveal() {
    let settled = Spring::new(1.0);
    assert!(presentation_anim_pending(&settled, 1.0, true, false));
    assert!(!presentation_anim_pending(&settled, 1.0, false, false));
    // A reveal still in flight keeps the loop alive even with no avatar.
    let moving = Spring::new(0.2);
    assert!(presentation_anim_pending(&moving, 1.0, false, false));
}

#[test]
fn escape_peels_the_tray_menu_before_the_panel() {
    let mut panel = CommandPanel::without_sources();
    panel.set_reduced_motion(true);
    let mut out = ChromeEvents::default();
    panel.toggle_command_panel(&mut out);
    panel.menu_open_for = Some("org.example.Tray".to_string());
    panel.menu_path = vec![0];

    panel.key_char(&escape(), &mut out);
    assert!(
        panel.menu_open_for.is_none(),
        "first Escape closes the menu"
    );
    assert!(panel.open, "panel stays open");

    panel.key_char(&escape(), &mut out);
    assert!(!panel.open, "second Escape closes the panel");
}

#[test]
fn selecting_another_tab_closes_the_tray_menu() {
    let mut panel = CommandPanel::without_sources();
    let module = panel
        .modules
        .metadata()
        .find(|module| module.availability == ModuleAvailability::Available)
        .expect("at least one available settings module");
    panel.menu_open_for = Some("org.example.Tray".to_string());
    panel.menu_path = vec![0];

    panel.select_tab(Tab::Settings(module.id));
    assert!(panel.menu_open_for.is_none());
    assert!(panel.menu_path.is_empty());
}

#[test]
fn settings_actions_coalesce_per_variant() {
    let set_idle = || SettingsAction::SetIdle {
        settings: tessera_desktop::settings::IdleSettings::default(),
    };
    let set_preferences = || SettingsAction::SetDesktopPreferences {
        preferences: tessera_desktop::settings::DesktopPreferences::default(),
    };
    assert!(same_action_kind(&set_idle(), &set_idle()));
    assert!(same_action_kind(&set_preferences(), &set_preferences()));
    assert!(!same_action_kind(&set_idle(), &set_preferences()));
}

#[test]
fn a_fullscreen_window_closes_the_panel() {
    let mut panel = CommandPanel::without_sources();
    panel.set_reduced_motion(true);
    let mut out = ChromeEvents::default();
    panel.toggle_command_panel(&mut out);
    panel.menu_open_for = Some("org.example.Tray".to_string());

    panel.update_windows(&[fullscreen_window()]);
    assert!(!panel.open);
    assert!(panel.menu_open_for.is_none());
    // The panel keeps the chrome layer for the ring drain after closing, then
    // releases it.
    for _ in 0..(SETTLED_DRAIN_FRAMES + 1) {
        panel.advance(0.016);
    }
    assert!(!panel.command_panel_active());
}

#[test]
fn cluster_bounds_stay_inside_small_displays() {
    for display in [(320.0, 480.0), (800.0, 600.0), (1920.0, 1080.0)] {
        let (profile, main, notifications, clock, tray, work_mode, power) =
            CommandPanel::cluster_bounds(display);
        for rect in [
            profile,
            main,
            notifications,
            clock,
            tray,
            work_mode,
            power,
        ] {
            assert!(rect.x >= 0.0 && rect.y >= 0.0);
            assert!(rect.x + rect.w <= display.0 + 0.01);
            assert!(rect.y + rect.h <= display.1 + 0.01);
        }
        assert!(notifications.x >= profile.x + profile.w);
        assert_eq!(notifications.y, profile.y);
    }
}

#[test]
fn full_size_displays_get_the_design_geometry() {
    let (profile, main, notifications, clock, tray, work_mode, power) =
        CommandPanel::cluster_bounds((1920.0, 1080.0));
    // Profile is compact in top-left
    assert_eq!(profile.w, 300.0);
    assert_eq!(profile.h, 84.0);
    assert_eq!(profile.x, 48.0);
    assert_eq!(profile.y, 37.8);

    // Notifications is compact in top-right
    assert_eq!(notifications.w, 260.0);
    assert_eq!(notifications.h, 200.0);
    assert_eq!(notifications.x, 1920.0 - 48.0 - 260.0);
    assert_eq!(notifications.y, 37.8);

    // The clock sits at top-center, clear of both top corners.
    assert!(clock.x > profile.x + profile.w);
    assert!(clock.x + clock.w < notifications.x);
    assert_eq!(clock.y, profile.y);

    // The tray column hugs the left-middle anchor.
    assert_eq!(tray.x, 48.0);
    assert!(tray.y > notifications.y);
    assert!(tray.h < 1080.0 - 37.8 * 2.0);

    // The right-bottom band is horizontal and pinned into the corner:
    // power/session flush to the right margin, work mode to its left, both
    // flush to the bottom margin and sharing one height.
    let margin_y = 37.8;
    assert_eq!(power.x + power.w, 1920.0 - 48.0);
    assert_eq!(power.y + power.h, 1080.0 - margin_y);
    assert_eq!(work_mode.x + work_mode.w + PANEL_GAP, power.x);
    assert_eq!(work_mode.y + work_mode.h, power.y + power.h);
    assert_eq!(work_mode.h, power.h);
    assert!(work_mode.y > notifications.y + notifications.h);

    // The main surface and clock share the screen's true centre axis. The
    // bottom band does not squeeze a panel that sits well above it.
    assert_eq!(main.w, MAIN_W);
    assert_eq!(main.h, MAIN_H);
    assert_eq!(main.x + main.w * 0.5, 1920.0 * 0.5);
    assert_eq!(clock.x + clock.w * 0.5, main.x + main.w * 0.5);
}

#[test]
fn profile_resolves_for_the_current_process_user() {
    let profile = Profile::current().expect("passwd record for the test user");
    assert!(!profile.username.is_empty());
    assert!(!profile.display_name.is_empty());
    assert!(!profile.initials.is_empty());
    // The primary group is always part of the list when group lookup works.
    assert!(!profile.groups.is_empty());
}

#[test]
fn clock_strings_render_a_time_and_a_date() {
    let (time, date) = crate::components::control_center::presentation::clock_strings();
    assert!(time.contains(':'), "wall-clock time renders as HH:MM");
    assert!(!date.is_empty(), "the date line renders");
}

#[test]
fn stagger_delays_the_content_panel_behind_the_menu() {
    assert_eq!(stagger(0.0, CONTENT_STAGGER), 0.0);
    assert_eq!(stagger(CONTENT_STAGGER, CONTENT_STAGGER), 0.0);
    assert!(stagger(0.5, 0.0) > stagger(0.5, CONTENT_STAGGER));
    assert_eq!(stagger(1.0, CONTENT_STAGGER), 1.0);
}

#[test]
fn control_center_paints_its_own_canvas_and_declares_no_backdrop() {
    let mut panel = ControlCenter::without_sources();
    let display = (1920.0, 1080.0);
    let workspaces = WorkspaceSnapshot {
        outputs: Vec::new(),
    };
    let mut out = ChromeEvents::default();

    // The panel never declares a backdrop capture, blur, frost, or analytic
    // glass — its background is painted chrome (ADR-0176).
    assert!(
        panel
            .liquid_glass_regions(display, &[], &workspaces)
            .is_empty()
    );
    assert_eq!(panel.backdrop_blur_sigma(), 0.0);
    assert!(panel.backdrop_regions(display, &[], &workspaces).is_empty());

    // Opening leaves the backdrop declarations empty: no full-screen blur is
    // ever requested, so the compositor's desktop capture/effect graph stays
    // off for the panel's whole lifecycle.
    panel.toggle_command_panel(&mut out);
    panel.reveal.snap_to(1.0);
    assert_eq!(
        panel.backdrop_blur_sigma(),
        0.0,
        "the panel must not request a full-screen blur"
    );
    assert!(panel.backdrop_regions(display, &[], &workspaces).is_empty());
}

#[test]
fn canvas_alpha_tracks_the_reveal_and_paints_nothing_at_zero() {
    // The painted canvas is opaque at full reveal and drains to nothing at the
    // start of an open / the end of a close, so it never pops.
    assert_eq!(canvas_alpha(0.0), 0, "a drained canvas paints nothing");
    assert_eq!(canvas_alpha(1.0), 255, "the settled canvas is opaque");
    assert!(
        canvas_alpha(0.5) > 0 && canvas_alpha(0.5) < 255,
        "the canvas rides the reveal: {}",
        canvas_alpha(0.5)
    );
    // Out-of-range progress clamps rather than overflowing the byte.
    assert_eq!(canvas_alpha(-0.5), 0);
    assert_eq!(canvas_alpha(1.5), 255);
}

#[test]
fn control_center_palette_tracks_the_live_appearance() {
    let mut panel = ControlCenter::without_sources();
    let dark = panel.panel_colors();
    assert!(!dark.is_light());

    panel.design = Design::light();
    let light = panel.panel_colors();
    assert!(light.is_light());
    assert_ne!(light.background, dark.background);
    assert_ne!(light.surface, dark.surface);
    assert_ne!(light.text, dark.text);
    assert_eq!(light.background.components().3, 255);
    assert_eq!(light.surface.components().3, 255);
}

#[test]
fn power_mode_projection_covers_the_four_toggle_combinations() {
    use tessera_desktop::power::PowerMode;
    // Display axis × security axis, the four user scenarios:
    // (keep awake, auto lock) → mode.
    assert_eq!(
        crate::components::control_center::presentation::power_mode_for(false, true),
        PowerMode::Balanced
    );
    assert_eq!(
        crate::components::control_center::presentation::power_mode_for(true, true),
        PowerMode::Secure
    );
    assert_eq!(
        crate::components::control_center::presentation::power_mode_for(true, false),
        PowerMode::Awake
    );
    // The forbidden combination (blank while never locking) projects onto
    // Awake: the security boundary wins and the display axis reads back
    // honestly as "awake".
    assert_eq!(
        crate::components::control_center::presentation::power_mode_for(false, false),
        PowerMode::Awake
    );
}

#[test]
fn projected_modes_never_blank_an_unlocked_session() {
    for keep_awake in [false, true] {
        for auto_lock in [false, true] {
            let mode = crate::components::control_center::presentation::power_mode_for(
                keep_awake, auto_lock,
            );
            if !mode.locks_automatically() {
                assert!(
                    !mode.blanks_display(),
                    "{mode:?} would blank a never-locking session"
                );
            }
        }
    }
}

#[test]
fn work_mode_and_power_session_panels_render_within_cluster() {
    let display = (1920.0, 1080.0);
    let (_profile, _main, notifications, _clock, _tray, work_mode, power) =
        CommandPanel::cluster_bounds(display);

    assert!(work_mode.w > 0.0);
    assert!(work_mode.h > 0.0);
    assert!(power.w > 0.0);
    assert!(power.h > 0.0);

    // The right-bottom band is one horizontal row sharing a height, clear
    // below the notification stream.
    assert!(work_mode.y > notifications.y + notifications.h);
    assert_eq!(work_mode.y, power.y);
    assert_eq!(work_mode.h, power.h);
    assert!(power.x > work_mode.x + work_mode.w);
}

#[test]
fn request_system_confirm_dedupes_while_one_is_pending() {
    let mut panel = CommandPanel::without_sources();
    let mut out = ChromeEvents::default();

    // The first destructive request latches and leaves through the events.
    panel.request_system_confirm(SystemAction::PowerOff, &mut out);
    assert_eq!(panel.power_pending_confirm, Some(SystemAction::PowerOff));
    assert_eq!(out.system_actions.len(), 1);

    // A second request while the consent dialog is still resolving is
    // dropped, not stacked.
    panel.request_system_confirm(SystemAction::Reboot, &mut out);
    assert_eq!(out.system_actions.len(), 1);
    assert_eq!(panel.power_pending_confirm, Some(SystemAction::PowerOff));

    // Closing the panel clears the latch so a reopen accepts a new action.
    panel.close();
    assert_eq!(panel.power_pending_confirm, None);
}

#[test]
fn scrollbar_reveals_fade_out_after_wheel_activity_stops() {
    let mut panel = CommandPanel::without_sources();
    panel.notif_scrollbar_reveal = 1.0;
    panel.tray_scrollbar_reveal = 1.0;

    // A handful of 60fps frames of decay settles both reveals to zero.
    for _ in 0..120 {
        panel.advance(1.0 / 60.0);
    }
    assert_eq!(panel.notif_scrollbar_reveal, 0.0);
    assert_eq!(panel.tray_scrollbar_reveal, 0.0);
    // And with nothing moving, no interaction animation stays pending.
    assert!(!panel.interaction_anim_pending());
}

#[test]
fn animated_frames_localize_damage_to_the_moving_bands() {
    let display = (1920.0, 1080.0);
    let (profile, _main, notifications, _clock, tray, work_mode, power) =
        CommandPanel::cluster_bounds(display);
    let full = tessera_primitives::Rect::new(0, 0, display.0 as i32, display.1 as i32);

    // Nothing animating: no footprint at all.
    assert_eq!(
        animated_damage_region(display, false, false, false, false, false, false, false),
        None
    );

    // The reveal transition stays conservative (None => full repaint) because
    // it animates the full-screen backdrop cover.
    assert_eq!(
        animated_damage_region(display, true, false, false, false, false, false, false),
        None
    );

    // A ticking work-mode spring repaints only its band, never the screen.
    let region = animated_damage_region(display, false, true, false, false, false, false, false)
        .expect("a ticking spring states its footprint");
    assert!(
        region.size.w < full.size.w && region.size.h < full.size.h,
        "the spring footprint must be a band: {region:?}"
    );
    assert!(region.origin.y >= work_mode.y as i32);

    // The notification-scrollbar reveal covers only the notification stream.
    let notif = animated_damage_region(display, false, false, true, false, false, false, false)
        .expect("scrollbar reveal states its footprint");
    assert!(notif.origin.x >= notifications.x as i32);
    assert!(notif.origin.x + notif.size.w <= (notifications.x + notifications.w).ceil() as i32);

    // The tray-scrollbar reveal covers only the tray column.
    let tray_region = animated_damage_region(display, false, false, false, true, false, false, false)
        .expect("tray reveal states its footprint");
    assert!(tray_region.origin.x >= tray.x as i32);
    assert!(tray_region.origin.x + tray_region.size.w <= (tray.x + tray.w).ceil() as i32);

    // An animating avatar narrows to the profile block, never the full output.
    let avatar = animated_damage_region(display, false, false, false, false, true, false, false)
        .expect("an animating avatar states its footprint");
    assert!(avatar.size.w < full.size.w && avatar.size.h < full.size.h);
    assert!(avatar.origin.x >= profile.x as i32);
    assert!(avatar.origin.x + avatar.size.w <= (profile.x + profile.w).ceil() as i32);

    // Tooltip reveals stay localized to their cluster anchor bands.
    let tip = animated_damage_region(display, false, false, false, false, false, true, false)
        .expect("work mode tooltip states localized footprint");
    assert!(tip.size.w < full.size.w && tip.size.h < full.size.h);

    let session_tip = animated_damage_region(display, false, false, false, false, false, false, true)
        .expect("session tooltip states localized footprint");
    assert!(session_tip.size.w < full.size.w && session_tip.size.h < full.size.h);
    assert!(session_tip.origin.x >= power.x as i32);

    // Independent signals union, so simultaneous motion still stays bounded
    // to the moving bands rather than the whole screen.
    let union = animated_damage_region(display, false, true, true, true, true, false, false).unwrap();
    assert!(union.size.w < full.size.w && union.size.h < full.size.h);
}

#[test]
fn tray_menu_opens_to_the_right_of_owner_and_equalizes_separator_height() {
    let owner = Rect {
        x: 48.0,
        y: 300.0,
        w: 56.0,
        h: 40.0,
    };
    let display = (1920.0, 1080.0);
    let items = vec![
        MenuNode {
            id: 1,
            kind: tray::MenuEntryKind::Standard,
            label: "Open".to_string(),
            enabled: true,
            visible: true,
            toggle: tray::MenuToggle::None,
            has_submenu: false,
            children: vec![],
        },
        MenuNode {
            id: 2,
            kind: tray::MenuEntryKind::Separator,
            label: String::new(),
            enabled: true,
            visible: true,
            toggle: tray::MenuToggle::None,
            has_submenu: false,
            children: vec![],
        },
        MenuNode {
            id: 3,
            kind: tray::MenuEntryKind::Standard,
            label: "Quit".to_string(),
            enabled: true,
            visible: true,
            toggle: tray::MenuToggle::None,
            has_submenu: false,
            children: vec![],
        },
    ];
    let bounds = menu_bounds(owner, &items, display);
    // Menu opens directly to the right of the tray column owner.
    assert!(
        bounds.x >= owner.x + owner.w,
        "popover x ({}) must be to the right of owner right edge ({})",
        bounds.x,
        owner.x + owner.w
    );
    // Separator contributes symmetric height (MENU_SEP_GAP * 2.0 + 1.0).
    let expected_h =
        MENU_PAD * 2.0 + MENU_HEADER_HEIGHT + 2.0 * MENU_ROW_HEIGHT + MENU_SEPARATOR_HEIGHT;
    assert_eq!(bounds.h, expected_h);
}

#[test]
fn demo_menu_state_is_provided_for_demo_keys() {
    let mut panel = CommandPanel::without_sources();
    panel.menu_open_for = Some("demo.agent".to_string());
    let menu = panel.menu_snapshot().expect("demo menu present");
    assert_eq!(menu.key, "demo.agent");
    assert!(!menu.root.children.is_empty());
}

#[test]
fn wifi_collapsed_and_expanded_state_lifecycle() {
    let mut panel = CommandPanel::without_sources();
    let mut out = ChromeEvents::default();

    panel.toggle_command_panel(&mut out);
    assert!(panel.open);
    assert!(!panel.wifi_expanded);

    // Expand Wi-Fi
    panel.wifi_expanded = true;
    assert!(panel.wifi_expanded);

    // Closing panel collapses Wi-Fi state
    panel.close();
    assert!(!panel.open);
    assert!(!panel.wifi_expanded);
}

#[test]
fn wifi_key_char_typing_and_connect() {
    let mut panel = CommandPanel::without_sources();
    let mut out = ChromeEvents::default();

    panel.open = true;
    panel.wifi_expanded = true;
    panel.wifi_input_ssid = Some("TestNet".to_string());

    let kc = |keysym, ch| KeyChar {
        keysym,
        ch,
        mods: tessera_primitives::input::Mods(0),
    };

    // Type "abc"
    panel.key_char(&kc('a' as u32, Some('a')), &mut out);
    panel.key_char(&kc('b' as u32, Some('b')), &mut out);
    panel.key_char(&kc('c' as u32, Some('c')), &mut out);
    assert_eq!(panel.wifi_input_passphrase, "abc");

    // Backspace pops 'c'
    panel.key_char(&kc(tessera_primitives::input::XKB_KEY_BackSpace, None), &mut out);
    assert_eq!(panel.wifi_input_passphrase, "ab");

    // Enter submits ConnectWifi action
    panel.key_char(&kc(tessera_primitives::input::XKB_KEY_Return, None), &mut out);
    assert_eq!(panel.wifi_input_ssid, None);
    assert!(panel.wifi_input_passphrase.is_empty());
    assert_eq!(out.system_actions.len(), 1);
    assert_eq!(
        out.system_actions[0],
        SystemAction::ConnectWifi {
            ssid: "TestNet".to_string(),
            passphrase: Some("ab".to_string()),
        }
    );
}

#[test]
fn wifi_escape_peels_expanded_view_first() {
    let mut panel = CommandPanel::without_sources();
    let mut out = ChromeEvents::default();

    panel.open = true;
    panel.wifi_expanded = true;

    let kc = |keysym| KeyChar {
        keysym,
        ch: None,
        mods: tessera_primitives::input::Mods(0),
    };

    // First escape collapses Wi-Fi detail, leaves panel open
    panel.key_char(&kc(tessera_primitives::input::XKB_KEY_Escape), &mut out);
    assert!(panel.open);
    assert!(!panel.wifi_expanded);

    // Second escape closes panel
    panel.key_char(&kc(tessera_primitives::input::XKB_KEY_Escape), &mut out);
    assert!(!panel.open);
}

#[test]
fn wifi_forget_and_autoconnect_actions_validate() {
    let mut out = ChromeEvents::default();
    out.system_actions.push(SystemAction::ForgetWifi {
        ssid: "OldCafe".to_string(),
    });
    out.system_actions.push(SystemAction::SetWifiAutoConnect {
        ssid: "MyPhoneHotspot".to_string(),
        auto_connect: false,
    });
    assert_eq!(out.system_actions.len(), 2);
    assert!(out.system_actions[0].validate().is_ok());
    assert!(out.system_actions[1].validate().is_ok());
}

#[test]
fn bluetooth_collapsed_and_expanded_state_lifecycle() {
    let mut panel = CommandPanel::without_sources();
    let mut out = ChromeEvents::default();

    panel.toggle_command_panel(&mut out);
    assert!(panel.open);
    assert!(!panel.bluetooth_expanded);

    // Expand Bluetooth
    panel.bluetooth_expanded = true;
    assert!(panel.bluetooth_expanded);

    // Closing the panel collapses the detail view.
    panel.close();
    assert!(!panel.open);
    assert!(!panel.bluetooth_expanded);
}

#[test]
fn bluetooth_and_wifi_detail_views_are_mutually_exclusive() {
    // The quick-controls body hosts one detail view at a time; opening either
    // tile's chevron clears the other.
    let mut panel = CommandPanel::without_sources();
    panel.open = true;

    panel.wifi_expanded = true;
    panel.select_tab(Tab::Settings(ModuleId::new("display")));
    // A tab switch clears both (the tile is not visible on a settings tab).
    assert!(!panel.wifi_expanded);
    assert!(!panel.bluetooth_expanded);
}

#[test]
fn bluetooth_escape_peels_expanded_view_first() {
    let mut panel = CommandPanel::without_sources();
    let mut out = ChromeEvents::default();

    panel.open = true;
    panel.bluetooth_expanded = true;

    let kc = |keysym| KeyChar {
        keysym,
        ch: None,
        mods: tessera_primitives::input::Mods(0),
    };

    // First escape collapses the Bluetooth detail, leaves the panel open.
    panel.key_char(&kc(tessera_primitives::input::XKB_KEY_Escape), &mut out);
    assert!(panel.open);
    assert!(!panel.bluetooth_expanded);

    // Second escape closes the panel.
    panel.key_char(&kc(tessera_primitives::input::XKB_KEY_Escape), &mut out);
    assert!(!panel.open);
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
    assert!(out.system_actions.iter().all(|action| action.validate().is_ok()));
}

#[test]
fn bluetooth_tile_subtitle_prioritizes_the_connected_device() {
    let i18n = Localizer::default();

    // No radio service: unavailable, never a fabricated "Off".
    assert_eq!(
        CommandPanel::bluetooth_tile_subtitle(&SystemStatus::default(), &i18n),
        i18n.text(Message::Unavailable)
    );

    // Radio off.
    let off = SystemStatus {
        bluetooth_enabled: Some(false),
        ..SystemStatus::default()
    };
    assert_eq!(
        CommandPanel::bluetooth_tile_subtitle(&off, &i18n),
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
        CommandPanel::bluetooth_tile_subtitle(&connected, &i18n),
        "Sony WH-1000XM5"
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
fn quick_controls_grid_fits_the_main_panel_body() {
    // The Bento grid sizes rows to their content: three tile rows (tile_h),
    // three fader/tier rows (fader_h), and the row gaps between them. This
    // arithmetic is the invariant the panel height must satisfy; a future
    // height tweak that breaks it would silently push the last fader row into
    // the panel's bottom rim (the original uniform-row-height defect).
    const TILE_H: f32 = 58.0;
    const FADER_H: f32 = 72.0;
    const GAP: f32 = 10.0;
    let grid_h = TILE_H * 3.0 + FADER_H * 3.0 + GAP * 5.0;

    // The body the Quick Controls section receives, mirroring
    // `render_main_panel`: panel height minus shell padding, the view header,
    // and the vertical padding.
    const SHELL_PAD: f32 = 12.0;
    const PAD_V: f32 = 10.0;
    const HEADER_H: f32 = 42.0;
    const HEADER_GAP: f32 = 8.0;
    let body_h = MAIN_H - SHELL_PAD * 2.0 - PAD_V * 2.0 - HEADER_H - HEADER_GAP;

    assert!(
        grid_h <= body_h,
        "Bento grid ({grid_h}px) must fit the panel body ({body_h}px)"
    );
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
    assert!(!indicator.anim_pending(0), "fresh indicator rests on tier 0");
    // Travel to the top tier; reduced motion resolves in a single frame.
    indicator.advance(3, 1.0 / 60.0, true);
    assert!(!indicator.anim_pending(3), "reduced motion resolves in one frame");
    assert!((indicator.spring.value - 3.0).abs() < 1e-3);
}

#[test]
fn indicator_spring_actually_overshoots() {
    // Regression: the indicator was tuned critically damped (ζ = 1.0), so the
    // "bounce" the comments and docs promised could not occur. The under-damped
    // tuning must cross its target before settling.
    assert!(
        crate::widgets::tiered::INDICATOR_SPRING.damping < 1.0,
        "the indicator must be under-damped to overshoot"
    );
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

#[test]
fn quick_controls_grid_is_placed_in_main_panel() {
    let mut panel = ControlCenter::without_sources();
    panel.open = true;
    panel.reveal.snap_to(1.0);
    let display = (1920.0, 1080.0);
    let workspaces = WorkspaceSnapshot {
        outputs: Vec::new(),
    };
    let windows = Vec::new();
    let i18n = Localizer::default();
    let mut out = ChromeEvents::default();

    let mut ui = lens::Ui::headless().unwrap();
    let mut input = Input::new(display, 0.05);

    let mut render_step = |input: &Input, out: &mut ChromeEvents| {
        ui.frame(input, |f| {
            panel.render(f, input, &windows, &workspaces, &i18n, out);
        });
        let snap = ui.snapshot().unwrap();
        ui.activate(&snap).unwrap();
    };

    // Initial neutral frame to resolve layout
    render_step(&input, &mut out);

    // Hovering near top-left (20, 20) must NOT hit the quick controls grid
    input.set_cursor(20.0, 20.0);
    render_step(&input, &mut out);
    assert!(out.system_actions.is_empty(), "quick controls must not drift to top-left (0, 0)");

    // Clicking inside the body area of the main panel targets the Wi-Fi toggle correctly.
    input.set_cursor(850.0, 380.0);
    input.set_mouse_down(lens::MouseButton::Left, true);
    input.set_mouse_pressed(lens::MouseButton::Left, true);
    input.set_mouse_released(lens::MouseButton::Left, false);
    render_step(&input, &mut out);

    input.set_mouse_down(lens::MouseButton::Left, false);
    input.set_mouse_pressed(lens::MouseButton::Left, false);
    input.set_mouse_released(lens::MouseButton::Left, true);
    render_step(&input, &mut out);

    assert!(!out.system_actions.is_empty(), "quick controls receives clicks at its placed location");
}
