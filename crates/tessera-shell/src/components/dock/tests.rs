//! Unit tests for the pure four-edge Dock, Hot Zone integration, Live Preview
//! Safe Corridor, and window-level Preview Card actions (ADR-0254, ADR-0255, ADR-0256).

use super::rendering::{
    PreviewCardAction, live_preview_layout, preview_card_action_hit, preview_card_close_rect,
    preview_card_pin_rect,
};
use super::*;
use crate::component::ChromeCommand;
use tessera_desktop::app::{ApplicationTarget, Entry};
use tessera_desktop::hot_zone::{HotZoneSlot, HotZoneSlotMask, HotZoneTrigger};
use tessera_desktop::window::{SpaceUse, Window, WindowId};

fn sample_entry(id: &str, name: &str) -> Entry {
    Entry {
        id: id.to_string(),
        name: name.to_string(),
        exec: Some(id.to_string()),
        icon: Some(id.to_string()),
        startup_wm_class: Some(id.to_string()),
        target: ApplicationTarget::External,
        ..Entry::default()
    }
}

fn sample_window(id: u64, app_id: &str, title: &str) -> Window {
    let mut w = Window::new(WindowId(id));
    w.app_id = Some(app_id.to_string());
    w.title = Some(title.to_string());
    w
}

#[test]
fn dock_declares_edge_only_hot_zone_capability() {
    let dock = Dock::new();
    let cap = dock
        .hot_zone_capability()
        .expect("Dock must advertise HotZoneCapability");
    assert_eq!(cap.allowed_slots, HotZoneSlotMask::EDGES);
    assert_eq!(cap.default_trigger, HotZoneTrigger::Hover);
    assert!(!cap.allowed_slots.contains(HotZoneSlot::TopLeft));
    assert!(cap.allowed_slots.contains(HotZoneSlot::Top));
    assert!(cap.allowed_slots.contains(HotZoneSlot::Bottom));
    assert!(cap.allowed_slots.contains(HotZoneSlot::Left));
    assert!(cap.allowed_slots.contains(HotZoneSlot::Right));
}

#[test]
fn pure_dock_rest_bounds_support_all_four_edges_symmetrically() {
    let display = (1920.0, 1080.0);
    let top = Dock::rest_bounds(4, 4, DockPosition::Top, display);
    let bottom = Dock::rest_bounds(4, 4, DockPosition::Bottom, display);
    let left = Dock::rest_bounds(4, 4, DockPosition::Left, display);
    let right = Dock::rest_bounds(4, 4, DockPosition::Right, display);

    // Top and Bottom are horizontal and identically sized.
    assert_eq!(top.w, bottom.w);
    assert_eq!(top.h, DOCK_PANEL_HEIGHT);
    assert_eq!(top.y, DOCK_EDGE_MARGIN);
    assert_eq!(bottom.y, display.1 - DOCK_PANEL_HEIGHT - DOCK_EDGE_MARGIN);
    assert!((top.x + top.w * 0.5 - display.0 * 0.5).abs() < 0.01);

    // Left and Right are vertical and match the horizontal bar's length.
    assert_eq!(left.h, top.w);
    assert_eq!(left.w, DOCK_PANEL_HEIGHT);
    assert_eq!(right.h, left.h);
    assert_eq!(left.x, DOCK_EDGE_MARGIN);
    assert_eq!(right.x, display.0 - DOCK_PANEL_HEIGHT - DOCK_EDGE_MARGIN);
}

#[test]
fn popup_side_opens_inward_for_all_four_edges() {
    assert_eq!(Dock::popup_side_for(DockPosition::Top), PopupSide::Below);
    assert_eq!(Dock::popup_side_for(DockPosition::Bottom), PopupSide::Above);
    assert_eq!(Dock::popup_side_for(DockPosition::Left), PopupSide::Right);
    assert_eq!(Dock::popup_side_for(DockPosition::Right), PopupSide::Left);
}

#[test]
fn hidden_autohide_dock_has_zero_footprint_and_summons_via_hot_zone_command() {
    let mut dock = Dock::new();
    dock.set_autohide(true);
    let display = (1920.0, 1080.0);
    let ws = tessera_desktop::workspace::WorkspaceSnapshot {
        outputs: Vec::new(),
    };

    // Hidden: zero composition, zero pointer capture, no body_rect.
    assert!(!dock.requires_composition());
    assert!(!dock.captures_pointer(960.0, 1078.0, display, &[], &ws));
    assert!(dock.body_rect(display).is_none());
    assert!(!dock.dock_expanded());

    // Summoned by HotZone command on the Top edge.
    let mut events = ChromeEvents::default();
    dock.command(&ChromeCommand::SummonDock(HotZoneSlot::Top), &mut events);
    assert_eq!(dock.position, DockPosition::Top);
    assert_eq!(dock.autohide_idle, 0.0);
    assert!(!dock.dismiss_latched);
    assert!(dock.anim_active);
}

#[test]
fn fullscreen_silences_dock_completely() {
    let mut dock = Dock::new();
    dock.apply_space_use(SpaceUse::Fullscreen);
    let display = (1920.0, 1080.0);
    let ws = tessera_desktop::workspace::WorkspaceSnapshot {
        outputs: Vec::new(),
    };

    assert!(!dock.requires_composition());
    assert!(!dock.captures_pointer(960.0, 1060.0, display, &[], &ws));
    assert_eq!(dock.reserved(), Reserved::default());
    assert!(dock.body_rect(display).is_none());
}

#[test]
fn multi_instance_preview_safe_corridor_protects_diagonal_transit() {
    let mut dock = Dock::new();
    dock.position = DockPosition::Bottom;
    let display = (1920.0, 1080.0);
    let owner = Rect {
        x: 932.0,
        y: 1006.0,
        w: 56.0,
        h: 56.0,
    };
    let windows = vec![WindowId(1), WindowId(2), WindowId(3), WindowId(4)];
    let presentation = live_preview_layout(
        &dock.design,
        display,
        owner,
        &windows,
        1.0,
        DockPosition::Bottom,
    );
    let panel = Rect {
        x: presentation.panel.origin.x as f32,
        y: presentation.panel.origin.y as f32,
        w: presentation.panel.size.w as f32,
        h: presentation.panel.size.h as f32,
    };
    dock.hover_owner_bounds = Some(owner);
    dock.hover_surface_bounds = Some(panel);
    dock.live_preview = Some(presentation);

    // Midpoint on a diagonal line from the owning icon centre to the far-left card.
    let start = (owner.x + owner.w * 0.5, owner.y + owner.h * 0.5);
    let far_left_target = (panel.x + 20.0, panel.y + panel.h - 4.0);
    let mid = (
        (start.0 + far_left_target.0) * 0.5,
        (start.1 + far_left_target.1) * 0.5,
    );

    assert!(
        dock.hover_surface_contains(mid.0, mid.1),
        "diagonal transit point {mid:?} must stay inside Safe Corridor"
    );

    // Aiming from `start` toward `mid` must engage Safe Triangle neighbour freeze.
    dock.last_cursor = Some(start);
    assert!(
        dock.pointer_aiming_at_preview(mid),
        "pointer moving diagonally toward wide preview must freeze neighbour switching"
    );
}

#[test]
fn preview_card_pin_and_close_actions_bind_to_exact_window_id() {
    let design = Design::dark();
    let display = (1920.0, 1080.0);
    let owner = Rect {
        x: 932.0,
        y: 1006.0,
        w: 56.0,
        h: 56.0,
    };
    let presentation = live_preview_layout(
        &design,
        display,
        owner,
        &[WindowId(10), WindowId(20)],
        1.0,
        DockPosition::Bottom,
    );
    let card2 = &presentation.cards[1];
    let outer = Rect {
        x: card2.geometry.outer.origin.x as f32,
        y: card2.geometry.outer.origin.y as f32,
        w: card2.geometry.outer.size.w as f32,
        h: card2.geometry.outer.size.h as f32,
    };
    let pin = preview_card_pin_rect(outer);
    let close = preview_card_close_rect(outer);

    assert_eq!(
        preview_card_action_hit(&presentation, pin.x + pin.w * 0.5, pin.y + pin.h * 0.5),
        Some(PreviewCardAction::ToggleAlwaysOnTop(WindowId(20)))
    );
    assert_eq!(
        preview_card_action_hit(
            &presentation,
            close.x + close.w * 0.5,
            close.y + close.h * 0.5
        ),
        Some(PreviewCardAction::Close(WindowId(20)))
    );
}

#[test]
fn edge_drag_target_recognizes_all_four_edges() {
    let display = (1920.0, 1080.0);
    assert_eq!(
        Dock::edge_drag_target((960.0, 10.0), display),
        Some(DockPosition::Top)
    );
    assert_eq!(
        Dock::edge_drag_target((960.0, 1070.0), display),
        Some(DockPosition::Bottom)
    );
    assert_eq!(
        Dock::edge_drag_target((10.0, 540.0), display),
        Some(DockPosition::Left)
    );
    assert_eq!(
        Dock::edge_drag_target((1910.0, 540.0), display),
        Some(DockPosition::Right)
    );
    assert_eq!(Dock::edge_drag_target((960.0, 540.0), display), None);
}

#[test]
fn running_windows_fold_into_pinned_app_tile_without_bento_cells() {
    let mut dock = Dock::new();
    let term = sample_entry("org.tessera.Term", "Terminal");
    dock.update_app_catalog(&AppCatalog {
        apps: vec![term.clone()],
        pinned: vec![term],
        icons: IconSet::default(),
        position: DockPosition::Bottom,
    });
    let w1 = sample_window(1, "org.tessera.Term", "Shell 1");
    let w2 = sample_window(2, "org.tessera.Term", "Shell 2");
    let tiles = dock.tiles(&[w1, w2]);
    assert_eq!(tiles.len(), 1);
    assert!(tiles[0].running);
    assert_eq!(tiles[0].windows, vec![WindowId(1), WindowId(2)]);
}
