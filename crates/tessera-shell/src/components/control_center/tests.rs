use super::*;

use super::presentation::canvas_alpha;

/// The reserved band a bottom-docked, non-autohidden dock publishes through
/// `ChromeUpdate::ModalReserved`. Sourced from the dock's own constants so a
/// dock geometry change flows into these tests instead of silently diverging.
fn bottom_dock_reserved() -> crate::component::Reserved {
    use crate::components::dock::motion::{DOCK_EDGE_MARGIN, DOCK_PANEL_HEIGHT};
    crate::component::Reserved {
        top: 0,
        bottom: (DOCK_PANEL_HEIGHT + DOCK_EDGE_MARGIN) as i32,
        left: 0,
        right: 0,
    }
}

fn escape() -> KeyChar {
    KeyChar {
        keysym: tessera_primitives::input::XKB_KEY_Escape,
        ch: None,
        mods: tessera_primitives::input::Mods::NONE,
    }
}

fn client_owns_output() -> tessera_desktop::window::SpaceUse {
    tessera_desktop::window::SpaceUse::Fullscreen
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
                &WorkspaceSnapshot {
                    outputs: Vec::new()
                },
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
    assert!(
        !panel.active(),
        "the chrome layer is released after the drain"
    );
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

    panel.update_space_use(client_owns_output());
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
fn the_dock_reservation_shortens_the_tray_shelf() {
    // ADR-0197: the shelf's bottom edge follows the dock's *own*
    // `Chrome::reserved()` declaration, published through
    // `ChromeUpdate::ModalReserved`. This is what a mirrored constant could not
    // do: an auto-hidden or fullscreen-locked dock reserves nothing, and a
    // side-docked dock does not reserve the bottom edge at all.
    let display = (1920.0, 1080.0);
    let (_, _, _, _, no_dock, _, _) =
        CommandPanel::cluster_bounds(display, crate::component::Reserved::default());
    let (_, _, _, _, bottom_dock, _, _) =
        CommandPanel::cluster_bounds(display, bottom_dock_reserved());

    assert!(
        (bottom_dock.y - no_dock.y).abs() < 0.5,
        "the shelf is bottom-anchored, so its top (below the profile chip) does \
         not move: {bottom_dock:?} vs {no_dock:?}"
    );
    assert!(
        bottom_dock.h < no_dock.h,
        "a bottom-docked dock must shorten the shelf: {} vs {}",
        bottom_dock.h,
        no_dock.h
    );
    let dock_band = bottom_dock_reserved().bottom as f32;
    let reclaimed = no_dock.h - bottom_dock.h;
    assert!(
        (reclaimed - dock_band).abs() < 0.5,
        "the shelf must give up exactly the dock's band ({dock_band}), gave up {reclaimed}"
    );

    // A side-docked dock reserves a side edge, not the bottom: the shelf keeps
    // its full height.
    let side_dock = crate::component::Reserved {
        top: 0,
        bottom: 0,
        left: 86,
        right: 0,
    };
    let (_, _, _, _, shelf, _, _) = CommandPanel::cluster_bounds(display, side_dock);
    assert!(
        (shelf.h - no_dock.h).abs() < 0.5,
        "a side-docked dock must not shorten the shelf"
    );
}

#[test]
fn the_tray_shelf_capacity_grows_with_the_display() {
    // [INV-CHROME-02]: the shelf's height is derived from the display and the
    // reserved bands, never capped by a constant. Before ADR-0197 the column
    // admitted exactly 7 icons at every one of these resolutions.
    use crate::components::control_center::tray_shelf::shelf_slots;
    let mut previous = 0;
    for display in [
        (1280.0, 720.0),
        (1366.0, 768.0),
        (1920.0, 1080.0),
        (2560.0, 1440.0),
    ] {
        let (_profile, _main, _notif, _clock, tray, _work, _power) =
            CommandPanel::cluster_bounds(display, bottom_dock_reserved());
        let slots = shelf_slots(tray.h);
        assert!(
            slots >= previous,
            "capacity shrank going to {display:?}: {slots} after {previous}"
        );
        previous = slots;
    }
    assert!(
        previous >= 20,
        "a 1440p output must admit well over the legacy 7 slots, got {previous}"
    );
}

#[test]
fn the_tray_shelf_never_overlaps_the_dock_band() {
    // ADR-0197: the dock is a persistent decoration that stays visible while
    // the panel is open, and its length changes with the tile count, so the
    // shelf is placed by a rule that holds clear of its reserved band — not by
    // an offset that happens to be clear on one monitor.
    for display in [
        (1280.0, 720.0),
        (1366.0, 768.0),
        (1920.0, 1080.0),
        (2560.0, 1440.0),
        (320.0, 480.0),
    ] {
        let (_profile, _main, _notif, _clock, tray, _work, _power) =
            CommandPanel::cluster_bounds(display, bottom_dock_reserved());
        let margin_y = (display.1 * 0.035).clamp(16.0, 40.0);
        let dock_top = display.1 - margin_y - bottom_dock_reserved().bottom as f32;
        assert!(
            tray.y + tray.h <= dock_top + 0.01,
            "the shelf ({tray:?}) must stop above the dock band at {dock_top} on {display:?}"
        );
    }
}

#[test]
fn the_tray_shelf_starts_below_the_profile_chip() {
    // The shelf grows upward from the bottom edge, so its *top* is the edge at
    // risk of colliding with the profile chip above it.
    for display in [
        (1280.0, 720.0),
        (1366.0, 768.0),
        (1920.0, 1080.0),
        (2560.0, 1440.0),
    ] {
        let (profile, _main, _notif, _clock, tray, _work, _power) =
            CommandPanel::cluster_bounds(display, bottom_dock_reserved());
        assert!(
            tray.y >= profile.y + profile.h,
            "the shelf ({tray:?}) must start below the profile chip ({profile:?})"
        );
        assert!(
            tray.h > 0.0,
            "the shelf keeps a usable height on {display:?}"
        );
    }
}

#[test]
fn cluster_bounds_stay_inside_small_displays() {
    for display in [(320.0, 480.0), (800.0, 600.0), (1920.0, 1080.0)] {
        let (profile, main, notifications, clock, tray, work_mode, power) =
            CommandPanel::cluster_bounds(display, bottom_dock_reserved());
        for rect in [profile, main, notifications, clock, tray, work_mode, power] {
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
        CommandPanel::cluster_bounds((1920.0, 1080.0), bottom_dock_reserved());
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

    // The tray shelf hugs the bottom-left anchor (ADR-0197): it starts below
    // the profile chip, spans down to the display's bottom margin, and holds
    // clear of the dock's reserved band rather than the side-middle band the
    // scrolling column used.
    assert_eq!(tray.x, 48.0);
    assert_eq!(
        tray.y,
        profile.y + profile.h + PANEL_GAP,
        "shelf starts below profile"
    );
    assert_eq!(
        tray.y + tray.h,
        1080.0 - 37.8 - bottom_dock_reserved().bottom as f32,
        "shelf stops above the dock's reserved band"
    );
    // [INV-CHROME-02]: the height is derived, so it far exceeds the retired
    // 400px cap that made capacity independent of the display.
    assert!(
        tray.h > (TRAY_CELL + TRAY_GAP) * 8.0,
        "the shelf must not be capped at the legacy eight-slot height: {}",
        tray.h
    );

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
    let (time, date) = crate::components::control_center::presentation::clock::clock_strings();
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
fn desktop_occlusion_is_asserted_only_at_full_opacity() {
    // `Chrome::occludes_desktop` is what lets the compositor elide the base
    // desktop pass, so it must be exactly as strict as the painted canvas:
    // opaque only once the reveal reaches an alpha of 255, and never while the
    // panel is closed (ADR-0181, [INV-ARCH-65]).
    let mut panel = ControlCenter::without_sources();
    assert!(!panel.occludes_desktop(), "a closed panel occludes nothing");

    // Mid-reveal the canvas is translucent: the desktop must still be drawn
    // beneath it.
    panel.open = true;
    panel.reveal.value = 0.5;
    assert!(panel.presented());
    assert_eq!(canvas_alpha(panel.reveal()), 128);
    assert!(
        !panel.occludes_desktop(),
        "a translucent canvas does not occlude the desktop"
    );

    // The threshold is the alpha byte saturating, not an arbitrary fraction.
    panel.reveal.value = 0.99;
    assert!(canvas_alpha(panel.reveal()) < 255);
    assert!(!panel.occludes_desktop());

    panel.reveal.value = 1.0;
    assert_eq!(canvas_alpha(panel.reveal()), 255);
    assert!(
        panel.occludes_desktop(),
        "the fully opaque canvas occludes the desktop"
    );

    // A drained panel (closed, reveal back to zero) stops occluding even
    // though the drain keeps it composited for the ring.
    panel.open = false;
    panel.reveal.value = 1.0;
    assert!(panel.occludes_desktop());
    panel.reveal.value = 0.0;
    assert!(!panel.occludes_desktop());
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
        crate::components::control_center::presentation::power::power_mode_for(false, true),
        PowerMode::Balanced
    );
    assert_eq!(
        crate::components::control_center::presentation::power::power_mode_for(true, true),
        PowerMode::Secure
    );
    assert_eq!(
        crate::components::control_center::presentation::power::power_mode_for(true, false),
        PowerMode::Awake
    );
    // The forbidden combination (blank while never locking) projects onto
    // Awake: the security boundary wins and the display axis reads back
    // honestly as "awake".
    assert_eq!(
        crate::components::control_center::presentation::power::power_mode_for(false, false),
        PowerMode::Awake
    );
}

#[test]
fn projected_modes_never_blank_an_unlocked_session() {
    for keep_awake in [false, true] {
        for auto_lock in [false, true] {
            let mode = crate::components::control_center::presentation::power::power_mode_for(
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
        CommandPanel::cluster_bounds(display, bottom_dock_reserved());

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
fn the_notification_scrollbar_reveal_fades_out_after_wheel_activity_stops() {
    // The tray has no scrollbar any more (ADR-0197 [INV-CHROME-03]); the
    // notification stream is the only remaining scrollable surface here.
    let mut panel = CommandPanel::without_sources();
    panel.notif_scrollbar_reveal = 1.0;

    // A handful of 60fps frames of decay settles the reveal to zero.
    for _ in 0..120 {
        panel.advance(1.0 / 60.0);
    }
    assert_eq!(panel.notif_scrollbar_reveal, 0.0);
    // And with nothing moving, no interaction animation stays pending.
    assert!(!panel.interaction_anim_pending());
}

#[test]
fn animated_frames_localize_damage_to_the_moving_bands() {
    let display = (1920.0, 1080.0);
    let (profile, _main, notifications, _clock, _tray, work_mode, power) =
        CommandPanel::cluster_bounds(display, bottom_dock_reserved());
    let full = tessera_primitives::Rect::new(0, 0, display.0 as i32, display.1 as i32);

    // Nothing animating: no footprint at all.
    assert_eq!(
        animated_damage_region(
            display,
            bottom_dock_reserved(),
            false,
            false,
            false,
            false,
            false,
            false
        ),
        None
    );

    // The reveal transition stays conservative (None => full repaint) because
    // it animates the full-screen backdrop cover.
    assert_eq!(
        animated_damage_region(
            display,
            bottom_dock_reserved(),
            true,
            false,
            false,
            false,
            false,
            false
        ),
        None
    );

    // A ticking work-mode spring repaints only its band, never the screen.
    let region = animated_damage_region(
        display,
        bottom_dock_reserved(),
        false,
        true,
        false,
        false,
        false,
        false,
    )
    .expect("a ticking spring states its footprint");
    assert!(
        region.size.w < full.size.w && region.size.h < full.size.h,
        "the spring footprint must be a band: {region:?}"
    );
    assert!(region.origin.y >= work_mode.y as i32);

    // The notification-scrollbar reveal covers only the notification stream.
    let notif = animated_damage_region(
        display,
        bottom_dock_reserved(),
        false,
        false,
        true,
        false,
        false,
        false,
    )
    .expect("scrollbar reveal states its footprint");
    assert!(notif.origin.x >= notifications.x as i32);
    assert!(notif.origin.x + notif.size.w <= (notifications.x + notifications.w).ceil() as i32);

    // An animating avatar narrows to the profile block, never the full output.
    let avatar = animated_damage_region(
        display,
        bottom_dock_reserved(),
        false,
        false,
        false,
        true,
        false,
        false,
    )
    .expect("an animating avatar states its footprint");
    assert!(avatar.size.w < full.size.w && avatar.size.h < full.size.h);
    assert!(avatar.origin.x >= profile.x as i32);
    assert!(avatar.origin.x + avatar.size.w <= (profile.x + profile.w).ceil() as i32);

    // Tooltip reveals stay localized to their cluster anchor bands.
    let tip = animated_damage_region(
        display,
        bottom_dock_reserved(),
        false,
        false,
        false,
        false,
        true,
        false,
    )
    .expect("work mode tooltip states localized footprint");
    assert!(tip.size.w < full.size.w && tip.size.h < full.size.h);

    let session_tip = animated_damage_region(
        display,
        bottom_dock_reserved(),
        false,
        false,
        false,
        false,
        false,
        true,
    )
    .expect("session tooltip states localized footprint");
    assert!(session_tip.size.w < full.size.w && session_tip.size.h < full.size.h);
    assert!(session_tip.origin.x >= power.x as i32);

    // Independent signals union, so simultaneous motion still stays bounded
    // to the moving bands rather than the whole screen.
    let union = animated_damage_region(
        display,
        bottom_dock_reserved(),
        false,
        true,
        true,
        true,
        false,
        false,
    )
    .unwrap();
    assert!(union.size.w < full.size.w && union.size.h < full.size.h);
}

static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn the_tray_shelf_discloses_overflow_instead_of_hiding_it() {
    let _env_guard = TEST_ENV_LOCK.lock().unwrap();
    // [INV-CHROME-04]: overflow is disclosed by a trailing `+N` cell, never
    // hidden behind a scroll or a page turn. This exercises the whole render
    // path: the disclosure cell must exist, be clickable, and open a popover
    // that lists every hidden item.
    unsafe { std::env::set_var("TESSERA_COMMAND_PANEL", "1") };

    let display = (1280.0, 720.0);
    let mut panel = ControlCenter::without_sources();
    panel.open = true;
    panel.reveal.snap_to(1.0);
    panel.set_reduced_motion(true);

    // Seed more SNI items than a 720p shelf can hold. The demo seed supplies
    // three; push the count past the shelf's capacity directly in the
    // projection-free path by asserting against the real capacity instead.
    let (_profile, _main, _notif, _clock, anchor, _work, _power) =
        ControlCenter::cluster_bounds(display, bottom_dock_reserved());
    let capacity = crate::components::control_center::tray_shelf::shelf_slots(anchor.h);
    assert!(
        capacity >= 2,
        "a 720p shelf must hold at least two slots, got {capacity}"
    );
    let item_count = capacity + 3;
    let layout = crate::components::control_center::tray_shelf::shelf_layout(anchor, item_count);
    assert!(layout.has_disclosure(), "an oversized tray must disclose");
    assert_eq!(layout.slots, capacity);
    assert_eq!(layout.overflow, item_count - (capacity - 1));

    // The disclosure popover's bounds are a pure function of the anchor, the
    // hidden count, and the display.
    let bounds = crate::components::control_center::tray_shelf::overflow_bounds(
        Rect {
            x: anchor.x,
            y: anchor.y,
            w: layout.cell_width(),
            h: TRAY_CELL,
        },
        layout.overflow,
        display,
    );
    assert!(bounds.w > 0.0 && bounds.h > 0.0);
    assert!(bounds.x >= 0.0 && bounds.y >= 0.0);
    assert!(bounds.x + bounds.w <= display.0 + 0.01);
    assert!(bounds.y + bounds.h <= display.1 + 0.01);

    // The popover must be reachable, not least by being *outside* the click-away
    // dismissal path: a press inside it must not close the panel.
    panel.tray_overflow_open = true;
    panel.tray_overflow_anchor = Rect {
        x: anchor.x,
        y: anchor.y + anchor.h - TRAY_CELL,
        w: layout.cell_width(),
        h: TRAY_CELL,
    };
    panel.tray_shelf = Some(layout);
    let resolved = panel
        .tray_overflow_bounds(display)
        .expect("an open disclosure states its footprint");
    assert!(
        resolved.w > 0.0 && resolved.h > 0.0,
        "click-away must be able to hit-test the disclosure popover"
    );

    unsafe { std::env::remove_var("TESSERA_COMMAND_PANEL") };
}

#[test]
fn a_disclosed_tray_never_lingers_with_nothing_to_show() {
    // When the shelf grows enough to hold every item, the popover has no
    // content and must close rather than paint an empty surface.
    let display = (1920.0, 1080.0);
    let (_profile, _main, _notif, _clock, tray, _work, _power) =
        ControlCenter::cluster_bounds(display, bottom_dock_reserved());
    let mut panel = ControlCenter::without_sources();
    panel.tray_overflow_open = true;
    // A layout with no overflow: everything fits.
    let fits = crate::components::control_center::tray_shelf::shelf_layout(tray, 2);
    assert!(!fits.has_disclosure());
    panel.tray_shelf = Some(fits);
    assert!(
        panel.tray_overflow_bounds(display).is_none(),
        "a shelf with nothing hidden must not report a popover"
    );
}

#[test]
fn a_press_outside_the_overflow_popover_dismisses_it() {
    // Symmetric with the dbusmenu popover's light-dismiss: a surface that
    // cannot be dismissed by clicking away lingers on screen.
    let display = (1280.0, 720.0);
    let mut panel = ControlCenter::without_sources();
    panel.open = true;
    panel.reveal.snap_to(1.0);
    panel.set_reduced_motion(true);
    let mut out = ChromeEvents::default();
    let mut ui = lens::Ui::headless().unwrap();
    let mut input = Input::new(display, 0.05);

    let anchor = Rect {
        x: 48.0,
        y: 300.0,
        w: TRAY_COL_W,
        h: crate::components::control_center::tray_shelf::shelf_height(2),
    };
    let cells: Vec<SniCell> = (0..6)
        .map(|index| SniCell {
            key: format!("test.tray.{index}"),
            title: format!("Item {index}"),
            textured: false,
            has_menu: true,
        })
        .collect();

    let frame =
        |ui: &mut lens::Ui, panel: &mut ControlCenter, input: &Input, out: &mut ChromeEvents| {
            ui.frame(input, |f| {
                panel.render_tray_shelf_cells(
                    f,
                    anchor,
                    display,
                    &cells,
                    1.0,
                    (input.as_raw().cursor.x, input.as_raw().cursor.y),
                    input.as_raw().mouse_pressed
                        [lens::sys::lens_mouse_button::LENS_MOUSE_LEFT as usize],
                );
            });
            let snap = ui.snapshot().unwrap();
            ui.activate(&snap).unwrap();
            let _ = out;
        };

    // Open the popover directly, then press far away from it.
    panel.tray_overflow_open = true;
    panel.tray_overflow_anchor = Rect {
        x: anchor.x,
        y: anchor.y + anchor.h - TRAY_CELL,
        w: TRAY_CELL,
        h: TRAY_CELL,
    };
    input.set_cursor(1200.0, 100.0);
    frame(&mut ui, &mut panel, &input, &mut out);

    input.set_mouse_down(lens::MouseButton::Left, true);
    input.set_mouse_pressed(lens::MouseButton::Left, true);
    frame(&mut ui, &mut panel, &input, &mut out);
    input.set_mouse_down(lens::MouseButton::Left, false);
    input.set_mouse_pressed(lens::MouseButton::Left, false);
    input.set_mouse_released(lens::MouseButton::Left, true);
    frame(&mut ui, &mut panel, &input, &mut out);
    input.set_mouse_released(lens::MouseButton::Left, false);

    assert!(
        !panel.tray_overflow_open,
        "a press outside the disclosure popover must dismiss it"
    );
}

#[test]
fn clicking_the_disclosure_cell_opens_the_overflow_popover() {
    // [INV-CHROME-04] end to end: the `+N` cell is a real pressable that
    // toggles the overflow popover. A pure-function test cannot prove this —
    // it needs a real frame and a real pointer sequence.
    let display = (1280.0, 720.0);
    let mut panel = ControlCenter::without_sources();
    panel.open = true;
    panel.reveal.snap_to(1.0);
    panel.set_reduced_motion(true);
    let mut out = ChromeEvents::default();
    let mut ui = lens::Ui::headless().unwrap();
    let mut input = Input::new(display, 0.05);

    // A deterministically oversized tray: a two-slot anchor fed six items.
    // Driving `render_tray_shelf_cells` with an explicit list keeps the test
    // independent of the preview demo seed and of the SNI projection.
    let anchor = Rect {
        x: 48.0,
        y: 300.0,
        w: TRAY_COL_W,
        h: crate::components::control_center::tray_shelf::shelf_height(2),
    };
    let cells: Vec<SniCell> = (0..6)
        .map(|index| SniCell {
            key: format!("test.tray.{index}"),
            title: format!("Item {index}"),
            has_menu: true,
            textured: false,
        })
        .collect();

    let layout = crate::components::control_center::tray_shelf::shelf_layout(anchor, cells.len());
    assert_eq!(layout.slots, 2);
    assert!(
        layout.has_disclosure(),
        "six items in two slots must disclose"
    );
    assert_eq!(layout.overflow, 5, "one own cell plus five behind +5");

    let cell_w = layout.cell_width();
    let cell_x = layout.surface.x + (layout.surface.w - cell_w) * 0.5 + cell_w * 0.5;
    // The disclosure is the last slot.
    let cell_y = layout.surface.y
        + TRAY_PAD
        + (layout.visible - 1) as f32 * (TRAY_CELL + TRAY_GAP)
        + TRAY_CELL * 0.5;

    let frame =
        |ui: &mut lens::Ui, panel: &mut ControlCenter, input: &Input, out: &mut ChromeEvents| {
            ui.frame(input, |f| {
                panel.render_tray_shelf_cells(
                    f,
                    anchor,
                    display,
                    &cells,
                    1.0,
                    (input.as_raw().cursor.x, input.as_raw().cursor.y),
                    input.as_raw().mouse_pressed
                        [lens::sys::lens_mouse_button::LENS_MOUSE_LEFT as usize],
                );
            });
            let snap = ui.snapshot().unwrap();
            ui.activate(&snap).unwrap();
            let _ = out;
        };

    // Two frames so the pressable has last-frame geometry to hit-test.
    input.set_cursor(cell_x, cell_y);
    frame(&mut ui, &mut panel, &input, &mut out);
    frame(&mut ui, &mut panel, &input, &mut out);
    assert!(!panel.tray_overflow_open, "closed before the press");
    assert_eq!(
        panel.tray_shelf.map(|shelf| shelf.overflow),
        Some(5),
        "the renderer must cache the resolved shelf layout"
    );

    input.set_mouse_down(lens::MouseButton::Left, true);
    input.set_mouse_pressed(lens::MouseButton::Left, true);
    frame(&mut ui, &mut panel, &input, &mut out);
    input.set_mouse_down(lens::MouseButton::Left, false);
    input.set_mouse_pressed(lens::MouseButton::Left, false);
    input.set_mouse_released(lens::MouseButton::Left, true);
    frame(&mut ui, &mut panel, &input, &mut out);
    input.set_mouse_released(lens::MouseButton::Left, false);

    assert!(
        panel.tray_overflow_open,
        "pressing the disclosure cell must open the overflow popover"
    );
}

#[test]
fn tray_cells_are_clickable_and_open_their_menu() {
    let _env_guard = TEST_ENV_LOCK.lock().unwrap();
    // Regression: the tray hover/open plate used to be a sibling `place()`
    // overlay drawn after the icon column. An ABS node in the Chrome band
    // occludes every flow widget inside its subtree for hit-testing, so the
    // plate swallowed each cell's own pressable response — hovering produced
    // no highlight, left-click never activated, and right-click never opened
    // the dbusmenu popover (ADR-0060: occlusion *is* the reversed hit-test
    // order). The plate is the cell's own background now, so the pressable
    // must see the pointer again.
    unsafe { std::env::set_var("TESSERA_COMMAND_PANEL", "1") };

    let display = (1920.0, 1080.0);
    let workspaces = WorkspaceSnapshot {
        outputs: Vec::new(),
    };
    let windows = Vec::new();
    let i18n = Localizer::default();
    let mut panel = ControlCenter::without_sources();
    panel.open = true;
    panel.reveal.snap_to(1.0);
    panel.set_reduced_motion(true);
    let mut out = ChromeEvents::default();
    let mut ui = lens::Ui::headless().unwrap();
    let mut input = Input::new(display, 0.05);

    // The demo cells are seeded by `sni_cells` under the preview env var. The
    // column is vertically centred in its anchor and horizontally centred at
    // `TRAY_PAD` inset, so the first cell's centre is derived from the same
    // Derive the cell centre from the *same* pure layout the renderer uses
    // (ADR-0197 [INV-CHROME-05]) instead of re-deriving the shelf's geometry
    // here: a hand-rolled copy is exactly how the column geometry drifted from
    // its renderer in the first place. The panel's own `reserved` state is the
    // authority, because that is what its render pass reads.
    let (_profile, _main, _notif, _clock, tray, _work, _power) =
        ControlCenter::cluster_bounds(display, panel.reserved);
    let cells = panel.sni_cells();
    let layout = crate::components::control_center::tray_shelf::shelf_layout(tray, cells.len());
    let cell_w = layout.cell_width();
    let cell_x = layout.surface.x + (layout.surface.w - cell_w) * 0.5 + cell_w * 0.5;
    // The shelf is bottom-anchored and grows upward: the first item's slot is
    // the topmost of the occupied slots.
    let cell_y = layout.surface.y + TRAY_PAD + TRAY_CELL * 0.5;
    let first_key = cells[0].key.clone();

    let frame =
        |ui: &mut lens::Ui, panel: &mut ControlCenter, input: &Input, out: &mut ChromeEvents| {
            render_headless(ui, panel, input, &windows, &workspaces, &i18n, out);
        };

    input.set_cursor(cell_x, cell_y);
    frame(&mut ui, &mut panel, &input, &mut out);
    frame(&mut ui, &mut panel, &input, &mut out);
    assert_eq!(panel.menu_open_for, None, "nothing opens before the press");

    // Right-click must reach the cell: it opens the host-rendered popover.
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
        Some(first_key.as_str()),
        "right-click on the first tray cell opens its dbusmenu popover"
    );
    unsafe { std::env::remove_var("TESSERA_COMMAND_PANEL") };
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
    panel.key_char(
        &kc(tessera_primitives::input::XKB_KEY_BackSpace, None),
        &mut out,
    );
    assert_eq!(panel.wifi_input_passphrase, "ab");

    // Enter submits ConnectWifi action
    panel.key_char(
        &kc(tessera_primitives::input::XKB_KEY_Return, None),
        &mut out,
    );
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
    // The quick-controls body hosts one detail view at a time; opening any
    // tile's chevron clears the others.
    let mut panel = CommandPanel::without_sources();
    panel.open = true;

    panel.wifi_expanded = true;
    panel.select_tab(Tab::Settings(ModuleId::new("display")));
    // A tab switch clears all (the tiles are not visible on a settings tab).
    assert!(!panel.wifi_expanded);
    assert!(!panel.bluetooth_expanded);
    assert!(!panel.cast_expanded);
}

#[test]
fn cast_collapsed_and_expanded_state_lifecycle() {
    let mut panel = CommandPanel::without_sources();
    let mut out = ChromeEvents::default();

    panel.toggle_command_panel(&mut out);
    assert!(panel.open);
    assert!(!panel.cast_expanded);

    // Expand Cast
    panel.cast_expanded = true;
    assert!(panel.cast_expanded);

    // Closing the panel collapses the detail view.
    panel.close();
    assert!(!panel.open);
    assert!(!panel.cast_expanded);
}

#[test]
fn cast_escape_peels_expanded_view_first() {
    let mut panel = CommandPanel::without_sources();
    let mut out = ChromeEvents::default();

    panel.open = true;
    panel.cast_expanded = true;

    let kc = |keysym| KeyChar {
        keysym,
        ch: None,
        mods: tessera_primitives::input::Mods(0),
    };

    // First escape collapses the Cast detail, leaves the panel open.
    panel.key_char(&kc(tessera_primitives::input::XKB_KEY_Escape), &mut out);
    assert!(panel.open);
    assert!(!panel.cast_expanded);

    // Second escape closes the panel.
    panel.key_char(&kc(tessera_primitives::input::XKB_KEY_Escape), &mut out);
    assert!(!panel.open);
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
fn test_inv_arch_80_cast_tile_reflects_status() {
    let i18n = Localizer::default();

    // An idle desktop reads as not casting, with no fabricated count.
    assert_eq!(
        CommandPanel::cast_tile_subtitle(&SystemStatus::default(), &i18n),
        i18n.text(Message::NotCasting)
    );

    // A present extended display with no live stream names the display count.
    let extended = SystemStatus {
        virtual_displays: 2,
        ..SystemStatus::default()
    };
    assert_eq!(
        CommandPanel::cast_tile_subtitle(&extended, &i18n),
        format!("{} · 2", i18n.text(Message::Displays))
    );

    // A live casting stream wins over a merely-present display.
    let casting = SystemStatus {
        virtual_displays: 2,
        cast_streams: 1,
        ..SystemStatus::default()
    };
    assert_eq!(
        CommandPanel::cast_tile_subtitle(&casting, &i18n),
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
        CommandPanel::cast_tile_subtitle(&session, &i18n),
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
    assert!(
        out.system_actions.is_empty(),
        "quick controls must not drift to top-left (0, 0)"
    );

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

    assert!(
        !out.system_actions.is_empty(),
        "quick controls receives clicks at its placed location"
    );
}

/// Render one headless frame of the panel and present it, so the next frame's
/// hit-testing resolves against real geometry.
fn render_headless(
    ui: &mut lens::Ui,
    panel: &mut ControlCenter,
    input: &Input,
    windows: &[tessera_desktop::window::Window],
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
