//! Public-boundary integration test for the overlay planning contract.
//!
//! This deliberately goes through `tessera_composite::presentation` rather than
//! the module-private unit tests: the composition root consumes exactly these
//! types, and a breaking change to the public contract must fail here.

use tessera_composite::presentation::{
    OverlayChange, OverlayPlacement, OverlayPlaneFacts, OverlaySceneFacts, OverlayState,
    OverlaySurfaceFacts, plan_overlay,
};
use tessera_desktop::window::WindowId;
use tessera_primitives::Rect;
use tessera_primitives::SurfaceDmabuf;

fn rect(x: i32, y: i32, w: i32, h: i32) -> Rect {
    Rect::new(x, y, w, h)
}

fn scene() -> OverlaySceneFacts {
    OverlaySceneFacts {
        candidate_is_topmost: true,
        dma_buf_surfaces_for_window: 1,
        output: rect(0, 0, 1920, 1080),
        ..Default::default()
    }
}

fn surface() -> OverlaySurfaceFacts {
    OverlaySurfaceFacts {
        window: WindowId(7),
        target: rect(100, 100, 640, 480),
        opaque: true,
        identity_mapped: true,
        scene_feedback: false,
        color_managed: false,
    }
}

fn candidate() -> SurfaceDmabuf {
    SurfaceDmabuf {
        id: 11,
        window: Some(WindowId(7)),
        width: 640,
        height: 480,
        generation: 1,
        damage: Vec::new(),
        damage_full: false,
        buffer_id: 1,
        fd: -1,
        drm_format: tessera_primitives::dmabuf::DRM_FORMAT_XRGB8888,
        modifier: 0,
        offset: 0,
        stride: 640 * 4,
        acquire_fence: -1,
        geometry: tessera_primitives::SurfaceGeometry::default(),
        opaque_region: None,
        color: None,
        presentation_hint: tessera_primitives::PresentationHint::Vsync,
    }
}

fn plane() -> OverlayPlaneFacts {
    OverlayPlaneFacts {
        available: true,
        format_supported: true,
        modifier_supported: true,
        zpos: Some(2),
    }
}

#[test]
fn accepted_plan_becomes_a_placement_and_buffer_updates_do_not_repaint_primary() {
    let plan = plan_overlay(scene(), Some(surface()), Some(&candidate()), plane());
    let placement = OverlayPlacement::from_plan(&plan, "DP-1".into(), rect(100, 100, 640, 480))
        .expect("accepted plan must become a placement");
    assert_eq!(placement.window, WindowId(7));
    assert_eq!(placement.surface_id, 11);

    let mut state = OverlayState::default();
    assert_eq!(
        state.reconcile(Some(placement.clone())),
        OverlayChange::Activated
    );
    assert!(OverlayChange::Activated.requires_primary_repaint());

    let next_buffer = OverlayPlacement {
        window: placement.window,
        surface_id: 12,
        output: placement.output.clone(),
        target: placement.target,
        zpos: placement.zpos,
    };
    assert_eq!(state.reconcile(Some(next_buffer)), OverlayChange::Updated);
    assert!(!OverlayChange::Updated.requires_primary_repaint());
    assert_eq!(state.active().map(|active| active.surface_id), Some(12));
}

#[test]
fn movement_repaints_primary_and_rejected_plan_never_activates() {
    let plan = plan_overlay(scene(), Some(surface()), Some(&candidate()), plane());
    let placement =
        OverlayPlacement::from_plan(&plan, "DP-1".into(), rect(100, 100, 640, 480)).unwrap();
    let mut state = OverlayState::default();
    assert_eq!(
        state.reconcile(Some(placement.clone())),
        OverlayChange::Activated
    );

    let moved = OverlayPlacement {
        target: rect(300, 40, 640, 480),
        ..placement
    };
    assert_eq!(state.reconcile(Some(moved)), OverlayChange::Replaced);
    assert!(OverlayChange::Replaced.requires_primary_repaint());

    let mut rejected_scene = scene();
    rejected_scene.session_locked = true;
    let rejected = plan_overlay(rejected_scene, Some(surface()), Some(&candidate()), plane());
    assert!(
        OverlayPlacement::from_plan(&rejected, "DP-1".into(), rect(100, 100, 640, 480)).is_none()
    );
    assert_eq!(state.reconcile(None), OverlayChange::Deactivated);
    assert!(state.active().is_none());
}
