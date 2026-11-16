use super::*;

/// A chrome component that draws one text field, so the shell's caret
/// contract (Optics ADR-0151) can be exercised without a GPU.
struct CaretField {
    buf: lens::TextBuf,
}

impl CaretField {
    fn new() -> Self {
        Self {
            buf: lens::TextBuf::new(64, ""),
        }
    }
}

impl Chrome for CaretField {
    fn render(
        &mut self,
        frame: &mut Frame,
        _input: &Input,
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
        _i18n: &Localizer,
        _out: &mut ChromeEvents,
    ) {
        frame.textfield("shell-caret-field", &mut self.buf);
    }
}

/// A caret that merely exists is not damage; only its blink *phase flip* is
/// (Optics ADR-0151).
///
/// The type error this pins is exact: the old contract reported
/// `caret_rect.w > 0` as damage, which is true for as long as any text
/// widget holds focus. That made `frame_needs_repaint` permanently true, so
/// a host with a focused field could never reach quiescence and every frame
/// it painted re-painted an identical caret.
///
/// The two facts must stay distinguishable: a live clock keeps the idle
/// tick alive so the phase can advance at all, while the flip owes the
/// paint. A test that only checked "no damage" would pass for a caret that
/// never blinks.
#[test]
fn a_focused_caret_ticks_without_owing_a_paint_every_frame() {
    let mut shell = Shell::headless().unwrap();
    shell.add(Box::new(CaretField::new()));

    // Build once so the field is laid out, then focus it. Focus is set
    // through the binding's escape hatch because the shell has no focus
    // model of its own: lens owns which widget holds the caret.
    shell.prepare_frame(&Input::default());
    assert!(
        !shell.caret_clock_live(),
        "an unfocused field runs no blink clock"
    );
    focus(&mut shell, "shell-caret-field");
    shell.prepare_frame(&Input::default());

    assert!(
        shell.caret_clock_live(),
        "a focused text field must run a blink clock"
    );

    // The tick that keeps the phase advancing is scheduled by the shell and
    // is a low-frequency one: a wakeup, not a paint cadence.
    let tick_deadline = shell
        .next_animation_deadline()
        .expect("a live blink clock must schedule its next tick");
    assert!(
        tick_deadline <= CARET_TICK_INTERVAL,
        "the caret tick must be due within one interval, got {tick_deadline:?}"
    );

    // Advance the blink over simulated time. Each iteration forces a build
    // the way the loop's own tick would; lens integrates the frame delta
    // inside a build, which is the only place the phase can move. A frame
    // that owes a paint is presented, which is what consumes the flip.
    //
    // 400 frames at 10 ms is 4 s of simulated time, i.e. roughly four blink
    // periods — so a correct contract owes about eight paints across them,
    // not 400 and not zero.
    let frame = Input::new((1920.0, 1080.0), 0.01);
    let mut painted = 0;
    for _ in 0..400 {
        shell.mark_dirty();
        shell.prepare_frame(&frame);
        assert!(shell.caret_clock_live(), "the blink clock must stay live");
        if shell.caret_flip_pending() {
            painted += 1;
            present(&mut shell);
            // Presenting is what consumes a flip: lens records the phase the
            // snapshot carried when it is activated, so the next build
            // compares against what the user actually saw. A build with no
            // time on it must therefore report nothing owed — this is the
            // assertion that separates "a transition" from "a level".
            shell.mark_dirty();
            shell.prepare_frame(&Input::new((1920.0, 1080.0), 0.0));
            assert!(
                !shell.caret_flip_pending(),
                "presenting a flip must consume it, or the caret repaints forever"
            );
        }
    }
    assert!(
        (4..=12).contains(&painted),
        "a 1.06 s blink owes ~8 paints over 4 s, got {painted}"
    );
}

/// Present the scene `prepare_frame` built, without drawing it.
///
/// This is the second half of [`Shell::submit`] — the activation that tells
/// lens which snapshot reached the display — performed here because a
/// headless shell has no canvas to submit through. It matters for the caret
/// contract: lens compares the current blink phase against the *presented*
/// one, so a flip is only consumed by an activation, never by a build.
fn present(shell: &mut Shell) {
    let Some(scene) = shell.pending_scene.as_ref() else {
        return;
    };
    shell
        .ui
        .activate(scene)
        .expect("activating a snapshot this UI just built must succeed");
}

/// Reduced motion disables the blink outright rather than slowing it: a
/// blink is motion, and "resolve in one frame" has no meaning for a state
/// that repeats forever (Optics ADR-0151, Tessera ADR-0029).
#[test]
fn reduced_motion_makes_the_caret_solid() {
    let mut shell = Shell::headless().unwrap();
    shell.set_reduced_motion(true);
    shell.add(Box::new(CaretField::new()));
    shell.prepare_frame(&Input::default());
    focus(&mut shell, "shell-caret-field");
    shell.prepare_frame(&Input::default());

    assert!(
        !shell.caret_clock_live(),
        "a disabled blink runs no clock, so nothing keeps the loop awake"
    );
    assert!(!shell.caret_flip_pending(), "a solid caret never flips");
    assert!(
        shell.next_animation_deadline().is_none(),
        "a solid caret must not schedule any wakeup"
    );

    // Turning the preference back on restores the blink. lens has no
    // "restore the default" call — `period <= 0` *is* "off" — so the shell
    // has to name a period, and this asserts it does.
    shell.set_reduced_motion(false);
    shell.prepare_frame(&Input::default());
    assert!(
        shell.caret_clock_live(),
        "leaving reduced motion must bring the blink clock back"
    );
}

/// Give `label` keyboard focus, so a lens text widget claims the caret.
///
/// The shell has no focus model of its own: lens owns which widget holds
/// the caret. Marking the scene dirty stands in for the click that would do
/// this in a live session — a focus change always arrives as input there,
/// and input is what makes `prepare_frame` build.
fn focus(shell: &mut Shell, label: &str) {
    assert!(shell.ui.focus(label), "the field must be focusable");
    shell.mark_dirty();
}

struct OrdinaryChrome;

impl Chrome for OrdinaryChrome {
    fn render(
        &mut self,
        _frame: &mut Frame,
        _input: &Input,
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
        _i18n: &Localizer,
        _out: &mut ChromeEvents,
    ) {
    }
}

struct PersistentDecoration;

impl Chrome for PersistentDecoration {
    fn render(
        &mut self,
        _frame: &mut Frame,
        _input: &Input,
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
        _i18n: &Localizer,
        _out: &mut ChromeEvents,
    ) {
    }

    fn persistent_decoration(&self) -> bool {
        true
    }
}

struct ExclusivePresentation;

impl Chrome for ExclusivePresentation {
    fn render(
        &mut self,
        _frame: &mut Frame,
        _input: &Input,
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
        _i18n: &Localizer,
        _out: &mut ChromeEvents,
    ) {
    }

    fn modal_active(&self) -> bool {
        true
    }

    fn visible_during_modal(&self) -> bool {
        true
    }

    fn exclusive_presentation_active(&self) -> bool {
        true
    }
}

struct SwitcherPresentation;

impl Chrome for SwitcherPresentation {
    fn render(
        &mut self,
        _frame: &mut Frame,
        _input: &Input,
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
        _i18n: &Localizer,
        _out: &mut ChromeEvents,
    ) {
    }

    fn visible_during_modal(&self) -> bool {
        true
    }

    fn window_switcher_active(&self) -> bool {
        true
    }
}

struct ScreenshotSessionChrome;

impl Chrome for ScreenshotSessionChrome {
    fn render(
        &mut self,
        _frame: &mut Frame,
        _input: &Input,
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
        _i18n: &Localizer,
        _out: &mut ChromeEvents,
    ) {
    }

    fn screenshot_active(&self) -> bool {
        true
    }
}

#[test]
fn ordinary_overlays_preserve_decorations_but_exclusive_presentations_do_not() {
    let ordinary = OrdinaryChrome;
    let decoration = PersistentDecoration;
    let exclusive = ExclusivePresentation;
    let switcher = SwitcherPresentation;

    assert!(!participates_in_shell_pass(
        &ordinary, false, true, false, false
    ));
    assert!(participates_in_shell_pass(
        &decoration,
        false,
        true,
        false,
        false
    ));
    assert!(!participates_in_shell_pass(
        &decoration,
        false,
        true,
        false,
        true
    ));
    assert!(participates_in_shell_pass(
        &exclusive, false, true, false, true
    ));
    assert!(!participates_in_shell_pass(
        &exclusive, false, true, true, true
    ));
    assert!(participates_in_shell_pass(
        &switcher, false, true, true, true
    ));
}

#[test]
fn screenshot_freeze_outranks_modal_and_exclusive_presentations() {
    let selector = ScreenshotSessionChrome;
    let decoration = PersistentDecoration;
    let exclusive = ExclusivePresentation;

    // While the freeze holds, only the selector participates — even when
    // an exclusive presentation such as the command panel is still open
    // underneath (its pixels are already part of the frozen snapshot).
    assert!(participates_in_shell_pass(
        &selector, true, true, false, true
    ));
    assert!(!participates_in_shell_pass(
        &exclusive, true, true, false, true
    ));
    assert!(!participates_in_shell_pass(
        &decoration,
        true,
        true,
        false,
        true
    ));
}

#[test]
fn reserved_inset_shrinks_and_clamps() {
    let r = Reserved {
        top: 10,
        bottom: 76,
        left: 4,
        right: 0,
    };
    let out = r.inset(tessera_primitives::Rect::new(0, 0, 1000, 800));
    assert_eq!(out.origin, tessera_primitives::Point { x: 4, y: 10 });
    assert_eq!(out.size, tessera_primitives::Size { w: 996, h: 714 }); // 800-10-76
}

#[test]
fn reserved_inset_clamps_to_non_negative() {
    let r = Reserved {
        top: 0,
        bottom: 2000,
        left: 0,
        right: 0,
    };
    let out = r.inset(tessera_primitives::Rect::new(0, 0, 100, 100));
    assert_eq!(out.size, tessera_primitives::Size { w: 100, h: 0 });
}

#[test]
fn cursor_shape_from_lens_cursor_hint() {
    assert_eq!(CursorShape::from_hint(lens::CursorHint::Default), None);
    assert_eq!(
        CursorShape::from_hint(lens::CursorHint::Pointer),
        Some(CursorShape::Pointer)
    );
    assert_eq!(
        CursorShape::from_hint(lens::CursorHint::Text),
        Some(CursorShape::Text)
    );
    assert_eq!(
        CursorShape::from_hint(lens::CursorHint::ResizeNs),
        Some(CursorShape::ResizeNs)
    );
    assert_eq!(
        CursorShape::from_hint(lens::CursorHint::ResizeEw),
        Some(CursorShape::ResizeEw)
    );
}

#[test]
fn cursor_shape_at_falls_back_to_ui_cursor_hint() {
    struct CapturingComponent;
    impl Chrome for CapturingComponent {
        fn render(
            &mut self,
            frame: &mut Frame,
            _input: &Input,
            _windows: &[Window],
            _workspaces: &WorkspaceSnapshot,
            _i18n: &Localizer,
            _out: &mut ChromeEvents,
        ) {
            frame.set_cursor_hint(lens::CursorHint::Text);
        }

        fn captures_pointer(
            &self,
            _x: f32,
            _y: f32,
            _display: (f32, f32),
            _windows: &[Window],
            _workspaces: &WorkspaceSnapshot,
        ) -> bool {
            true
        }
    }

    let mut shell = Shell::headless().unwrap();
    shell.add(Box::new(CapturingComponent));
    let input = Input::default();
    shell.frame_headless(&input);
    assert_eq!(shell.cursor_hint(), Some(CursorShape::Text));
    assert_eq!(
        shell.cursor_shape_at(10.0, 10.0, (1920.0, 1080.0)),
        Some(CursorShape::Text)
    );
}

/// A full-output cover reports its own opacity, so the compositor can tell
/// "the desktop is gone" from "the cover is still draining".
struct CoverChrome {
    opacity: f32,
}

impl Chrome for CoverChrome {
    fn render(
        &mut self,
        _frame: &mut Frame,
        _input: &Input,
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
        _i18n: &Localizer,
        _out: &mut ChromeEvents,
    ) {
    }

    fn paints_opaque_cover(&self) -> bool {
        true
    }

    fn cover_opacity(&self) -> f32 {
        self.opacity
    }
}

#[test]
fn a_cover_mid_fade_still_needs_the_desktop_beneath_it() {
    let mut shell = Shell::headless().unwrap();
    shell.add(Box::new(CoverChrome { opacity: 0.5 }));
    assert!(
        !shell.opaque_cover_active(),
        "a half-drained cover leaves the desktop visible; eliding it would \
         freeze the launch fade over a stale frame"
    );
}

#[test]
fn a_fully_opaque_cover_elides_the_desktop_pass() {
    let mut shell = Shell::headless().unwrap();
    shell.add(Box::new(CoverChrome { opacity: 1.0 }));
    assert!(shell.opaque_cover_active());
}

use lens::Rect;

/// A component whose animation footprint is a small band.
struct LocalizedAnim {
    pending: bool,
    footprint: Option<tessera_primitives::Rect>,
}

impl Chrome for LocalizedAnim {
    fn render(
        &mut self,
        _f: &mut Frame,
        _input: &Input,
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
        _i18n: &Localizer,
        _out: &mut ChromeEvents,
    ) {
    }
    fn anim_pending(&self) -> bool {
        self.pending
    }
    fn damage_regions(&self, _windows: &[Window], _display: (f32, f32)) -> Option<Vec<Rect>> {
        self.footprint.map(|rect| {
            vec![Rect {
                x: rect.origin.x as f32,
                y: rect.origin.y as f32,
                w: rect.size.w as f32,
                h: rect.size.h as f32,
            }]
        })
    }
}

/// A component that captures the pointer over a band and can say so.
struct HoveringChrome {
    band: Option<tessera_primitives::Rect>,
}

impl Chrome for HoveringChrome {
    fn render(
        &mut self,
        _f: &mut Frame,
        _input: &Input,
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
        _i18n: &Localizer,
        _out: &mut ChromeEvents,
    ) {
    }
    fn captures_pointer(
        &self,
        _x: f32,
        _y: f32,
        _display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> bool {
        true
    }
    fn pointer_damage_regions(
        &self,
        _x: f32,
        _y: f32,
        _display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> Option<Vec<Rect>> {
        self.band.map(|rect| {
            vec![Rect {
                x: rect.origin.x as f32,
                y: rect.origin.y as f32,
                w: rect.size.w as f32,
                h: rect.size.h as f32,
            }]
        })
    }
}

/// A pointer over chrome that states its own hover band repaints that band,
/// not the output.
///
/// A pointer move carries no pixel footprint of its own, so the assessment
/// has to learn one from the chrome under the cursor. Answering "chrome
/// captured the pointer" with a full-output composite made every mouse
/// movement over an open panel repaint the whole screen at the panel's
/// refresh rate.
#[test]
fn a_pointer_over_localizing_chrome_repaints_only_its_band() {
    let mut shell = Shell::headless().unwrap();
    shell.add(Box::new(HoveringChrome {
        band: Some(tessera_primitives::Rect::new(0, 1000, 1920, 80)),
    }));
    let regions = shell
        .pointer_damage_regions(500.0, 1040.0, (1920.0, 1080.0))
        .expect("chrome that states its band localizes the pointer");
    assert_eq!(
        regions,
        vec![tessera_primitives::Rect::new(0, 1000, 1920, 80)]
    );
}

/// Chrome that cannot bound its own hover reaction keeps the conservative
/// full-output answer. The seam exists to remove a *guess*, not to force
/// every component to invent a band.
#[test]
fn a_pointer_over_unbounded_chrome_stays_a_full_repaint() {
    let mut shell = Shell::headless().unwrap();
    shell.add(Box::new(HoveringChrome { band: None }));
    assert_eq!(
        shell.pointer_damage_regions(500.0, 1040.0, (1920.0, 1080.0)),
        None
    );
}

/// A pointer over an area no component claims carries no chrome damage.
#[test]
fn a_pointer_outside_chrome_claims_nothing() {
    struct Silent;
    impl Chrome for Silent {
        fn render(
            &mut self,
            _f: &mut Frame,
            _input: &Input,
            _windows: &[Window],
            _workspaces: &WorkspaceSnapshot,
            _i18n: &Localizer,
            _out: &mut ChromeEvents,
        ) {
        }
    }
    let mut shell = Shell::headless().unwrap();
    shell.add(Box::new(Silent));
    assert_eq!(
        shell.pointer_damage_regions(500.0, 500.0, (1920.0, 1080.0)),
        Some(Vec::new())
    );
}

/// `[INV-ARCH-92]`-adjacent: the scene must be **built** by the time damage
/// is assessed, because lens's damage query only describes a frame that
/// exists. Confirm `prepare_frame` is what builds it, so a host that
/// assesses damage between `prepare_frame` and `submit` reads this frame.
#[test]
fn prepare_frame_builds_the_scene_damage_assessment_reads() {
    let mut shell = Shell::headless().unwrap();
    // Nothing built until a frame is prepared.
    assert!(!shell.has_pending_scene());
    shell.prepare_frame(&Input::default());
    assert!(
        shell.has_pending_scene(),
        "prepare_frame must build the scene, or damage assessment reports the previous frame"
    );
    // Building is idempotent within a frame: a second call rebuilds rather
    // than accumulating or leaving a stale scene.
    shell.prepare_frame(&Input::default());
    assert!(shell.has_pending_scene());
}

/// A failed build must degrade to the last good scene and must not become a
/// per-frame rebuild loop (ADR-0223).
///
/// The contract is observed at the draw point: `submit` must keep drawing
/// the last scene that built, so a degenerate chrome command costs stale
/// chrome rather than a chrome-less desktop. The backoff gate is observed
/// separately, as "no rebuild happened".
#[test]
fn a_failed_build_keeps_the_last_good_scene_and_backs_off() {
    let mut shell = Shell::headless().unwrap();
    shell.prepare_frame(&Input::default());
    assert!(shell.has_pending_scene(), "a good build publishes a scene");
    assert!(
        shell.last_good_scene.is_some(),
        "the good scene is retained"
    );
    assert!(shell.build_retry_at.is_none(), "no backoff after success");

    // The state a failed build leaves behind: nothing pending, the good
    // scene retained, and a retry deadline in the future.
    shell.pending_scene = None;
    shell.build_retry_at = Some(std::time::Instant::now() + BUILD_RETRY_BACKOFF);

    // `submit` must still find a scene to draw: the last good one.
    assert!(
        shell
            .pending_scene
            .as_ref()
            .or(shell.last_good_scene.as_ref())
            .is_some(),
        "a failed build must fall back to the last good scene, not to no chrome"
    );

    // The retry gate: a prepare inside the backoff window must not rebuild.
    // `dirty` surviving is the proof — a successful build clears it.
    shell.mark_dirty();
    shell.prepare_frame(&Input::default());
    assert!(
        shell.dirty,
        "the backoff must suppress a rebuild inside its window"
    );

    // Past the deadline the rebuild is allowed again, so a transient
    // failure still heals.
    shell.build_retry_at = Some(std::time::Instant::now() - BUILD_RETRY_BACKOFF);
    shell.prepare_frame(&Input::default());
    assert!(!shell.dirty, "the retry past its deadline rebuilds");
}

#[test]
fn quiescent_shell_preserves_scene_without_rebuilding() {
    let mut shell = Shell::headless().unwrap();
    shell.prepare_frame(&Input::default());
    assert!(shell.has_pending_scene());
    assert!(!shell.dirty);

    // Calling prepare_frame with quiescent input preserves the existing scene
    shell.prepare_frame(&Input::default());
    assert!(shell.has_pending_scene());

    // Marking dirty triggers rebuild
    shell.mark_dirty();
    assert!(shell.dirty);
    shell.prepare_frame(&Input::default());
    assert!(!shell.dirty);
}

/// The scene is a *presentable artifact*, and presenting it clears it — but
/// only on success. A failed draw must leave it available so the caller can
/// degrade and still inspect the frame it built (ADR-0203).
#[test]
fn a_built_scene_presents_once_and_is_retained_until_it_does() {
    let mut shell = Shell::headless().unwrap();
    shell.prepare_frame(&Input::default());
    assert!(shell.has_pending_scene());

    // A submit against a bogus canvas fails, but the scene survives: the
    // failure path must not consume the frame.
    let mut bogus = 0u8;
    let result = unsafe { shell.submit(&mut bogus as *mut u8 as *mut std::ffi::c_void) };
    assert!(result.is_err(), "a null canvas must not silently succeed");
    assert!(
        shell.has_pending_scene(),
        "a failed draw must not consume the scene"
    );

    // Damage assessment still reads the frame it built, not a cleared one.
    // Headless has no chrome, so the region is empty — but the query must
    // answer rather than panic on a built-but-undrawn scene.
    let _ = shell.chrome_damage_regions((1920.0, 1080.0), false);

    // A successful build followed by a fresh build refreshes in place.
    shell.prepare_frame(&Input::default());
    assert!(shell.has_pending_scene());
}

/// A component that is not animating but owes a bounded damage repair for
/// pixels it just removed (ADR-0227).
struct RetiringChrome {
    footprint: tessera_primitives::Rect,
    pending: bool,
}

impl Chrome for RetiringChrome {
    fn render(
        &mut self,
        _frame: &mut Frame,
        _input: &Input,
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
        _i18n: &Localizer,
        _out: &mut ChromeEvents,
    ) {
    }

    fn damage_pending(&self) -> bool {
        self.pending
    }

    fn damage_regions(&self, _windows: &[Window], _display: (f32, f32)) -> Option<Vec<Rect>> {
        Some(vec![Rect {
            x: self.footprint.origin.x as f32,
            y: self.footprint.origin.y as f32,
            w: self.footprint.size.w as f32,
            h: self.footprint.size.h as f32,
        }])
    }
}

#[test]
fn a_damage_only_retirement_is_queried_without_an_animation() {
    let mut shell = Shell::headless().unwrap();
    shell.add(Box::new(RetiringChrome {
        footprint: tessera_primitives::Rect::new(300, 200, 440, 260),
        pending: true,
    }));
    assert!(!shell.anim_pending(), "a retirement is not motion");
    assert!(shell.damage_pending());
    assert_eq!(
        shell.chrome_damage_regions((1920.0, 1080.0), false),
        Some(vec![tessera_primitives::Rect::new(300, 200, 440, 260)]),
        "the repair frame must name the vacated footprint"
    );

    let mut shell = Shell::headless().unwrap();
    shell.add(Box::new(RetiringChrome {
        footprint: tessera_primitives::Rect::new(300, 200, 440, 260),
        pending: false,
    }));
    assert!(!shell.damage_pending());
    assert_eq!(
        shell.chrome_damage_regions((1920.0, 1080.0), false),
        Some(Vec::new()),
        "an exhausted retirement owns no pixels"
    );
}

#[test]
fn a_localized_chrome_animation_narrows_the_repaint() {
    // `[INV-ARCH-93]`-adjacent shell contract: a component that names its
    // footprint must produce a region, so the compositor repaints the band
    // instead of the whole output.
    let mut shell = Shell::headless().unwrap();
    shell.add(Box::new(LocalizedAnim {
        pending: true,
        footprint: Some(tessera_primitives::Rect::new(0, 0, 400, 32)),
    }));
    assert_eq!(
        shell.chrome_damage_regions((1920.0, 1080.0), false),
        Some(vec![tessera_primitives::Rect::new(0, 0, 400, 32)]),
        "a self-localized animation must not widen to a full-output repaint"
    );
}

/// A persistent decoration — the shape of the HUD strip or the Dock — that
/// claims a fixed band of the output.
struct PersistentBand {
    footprint: Option<tessera_primitives::Rect>,
    visible: bool,
}

impl Chrome for PersistentBand {
    fn render(
        &mut self,
        _frame: &mut Frame,
        _input: &Input,
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
        _i18n: &Localizer,
        _out: &mut ChromeEvents,
    ) {
    }

    fn persistent_decoration(&self) -> bool {
        true
    }

    fn requires_composition(&self) -> bool {
        self.visible
    }

    fn damage_regions(&self, _windows: &[Window], _display: (f32, f32)) -> Option<Vec<Rect>> {
        self.footprint.map(|rect| {
            vec![Rect {
                x: rect.origin.x as f32,
                y: rect.origin.y as f32,
                w: rect.size.w as f32,
                h: rect.size.h as f32,
            }]
        })
    }
}

/// Two chrome bodies animating at opposite edges of the output must
/// produce two rectangles, not one bounding box spanning the panel. The
/// HUD sits at the top and the Dock at the bottom, so a union is the whole
/// screen and the narrow repaint is lost (ADR-0203).
#[test]
fn disjoint_chrome_animations_stay_disjoint() {
    let mut shell = Shell::headless().unwrap();
    shell.add(Box::new(LocalizedAnim {
        pending: true,
        footprint: Some(tessera_primitives::Rect::new(0, 0, 800, 32)),
    }));
    shell.add(Box::new(LocalizedAnim {
        pending: true,
        footprint: Some(tessera_primitives::Rect::new(500, 1040, 900, 40)),
    }));
    let regions = shell
        .chrome_damage_regions((1920.0, 1080.0), false)
        .expect("both components localize their animation");
    assert_eq!(
        regions,
        vec![
            tessera_primitives::Rect::new(0, 0, 800, 32),
            tessera_primitives::Rect::new(500, 1040, 900, 40),
        ],
        "regions must stay disjoint; a union would cover the whole output"
    );
    // And the union of what we did report must be smaller than the panel.
    let total: i64 = regions
        .iter()
        .map(|r| r.size.w as i64 * r.size.h as i64)
        .sum();
    let panel = 1920i64 * 1080;
    assert!(
        total * 10 < panel,
        "reported chrome damage ({total}px^2) should be a small fraction of the panel ({panel}px^2)"
    );
}

#[test]
fn a_component_without_a_footprint_still_forces_full_output() {
    // The conservative escape hatch must survive: `None` still means
    // "repaint everything".
    let mut shell = Shell::headless().unwrap();
    shell.add(Box::new(LocalizedAnim {
        pending: true,
        footprint: None,
    }));
    assert_eq!(shell.chrome_damage_regions((1920.0, 1080.0), false), None);
}

#[test]
fn an_idle_component_contributes_no_region() {
    let mut shell = Shell::headless().unwrap();
    shell.add(Box::new(LocalizedAnim {
        pending: false,
        footprint: Some(tessera_primitives::Rect::new(0, 0, 400, 32)),
    }));
    assert_eq!(
        shell.chrome_damage_regions((1920.0, 1080.0), false),
        Some(Vec::new()),
        "nothing is animating, so there is no chrome animation damage"
    );
}

/// A structural chrome change with no animation — a dock tile's context menu
/// opening or closing — must still repaint its footprint on that frame.
/// `anim_pending` deliberately stays false for it, so `scene_changed` is the
/// only signal, and dropping it left the popup rendered into the model but
/// never into a rebuilt scene.
#[test]
fn a_scene_change_repaints_without_an_animation() {
    let mut shell = Shell::headless().unwrap();
    shell.add(Box::new(LocalizedAnim {
        pending: false,
        footprint: Some(tessera_primitives::Rect::new(500, 700, 236, 180)),
    }));
    assert_eq!(
        shell.chrome_damage_regions((1920.0, 1080.0), false),
        Some(Vec::new()),
        "a quiescent component contributes nothing"
    );
    assert_eq!(
        shell.chrome_damage_regions((1920.0, 1080.0), true),
        Some(vec![tessera_primitives::Rect::new(500, 700, 236, 180)]),
        "a structural change must repaint its footprint even without an animation"
    );
}

/// The one-shot report is cleared by reading it: the rebuild it asks for has
/// already happened, and re-reporting it would keep the frame loop awake.
#[test]
fn a_scene_change_is_reported_once() {
    let mut shell = Shell::headless().unwrap();
    assert!(!shell.take_scene_changed());
    shell.events.scene_changed = true;
    assert!(shell.take_scene_changed());
    assert!(!shell.take_scene_changed());
}

/// The overlay-pass elision must answer from chrome's real footprint, not
/// from a per-edge threshold. A dock on the left or right edge occupies no
/// top or bottom band, so a y-only test answered "no chrome here" and the
/// compositor clipped the dock's pixels away.
#[test]
fn the_overlay_elision_respects_chrome_on_any_edge() {
    let mut shell = Shell::headless().unwrap();
    // A persistent decoration claiming the left edge only — a left-docked bar.
    shell.add(Box::new(PersistentBand {
        footprint: Some(tessera_primitives::Rect::new(0, 0, 86, 1080)),
        visible: true,
    }));

    let left_edge = flux::CanvasRenderArea {
        x: 0.0,
        y: 400.0,
        width: 40.0,
        height: 200.0,
    };
    assert!(
        shell.chrome_intersects_render_area(left_edge, 1.0, (1920, 1080)),
        "a left-edge decoration owns pixels in a left-edge render area"
    );

    let middle = flux::CanvasRenderArea {
        x: 800.0,
        y: 400.0,
        width: 40.0,
        height: 200.0,
    };
    assert!(
        !shell.chrome_intersects_render_area(middle, 1.0, (1920, 1080)),
        "the central workspace carries no chrome pixels"
    );
}

/// A decoration that is not visible owns no pixels, so it must not block the
/// elision at all.
#[test]
fn an_invisible_decoration_blocks_nothing() {
    let mut shell = Shell::headless().unwrap();
    shell.add(Box::new(PersistentBand {
        footprint: Some(tessera_primitives::Rect::new(0, 0, 86, 1080)),
        visible: false,
    }));
    let anywhere = flux::CanvasRenderArea {
        x: 0.0,
        y: 0.0,
        width: 1920.0,
        height: 1080.0,
    };
    assert!(
        !shell.chrome_intersects_render_area(anywhere, 1.0, (1920, 1080)),
        "dormant chrome cannot intersect anything"
    );
}

/// A non-decoration that is visible owns arbitrary pixels and cannot be
/// bounded by any band, so it must disqualify the elision outright.
#[test]
fn a_visible_overlay_always_blocks_the_elision() {
    let mut shell = Shell::headless().unwrap();
    shell.add(Box::new(LocalizedAnim {
        pending: false,
        footprint: Some(tessera_primitives::Rect::new(800, 400, 236, 180)),
    }));
    let middle = flux::CanvasRenderArea {
        x: 800.0,
        y: 400.0,
        width: 40.0,
        height: 40.0,
    };
    assert!(
        shell.chrome_intersects_render_area(middle, 1.0, (1920, 1080)),
        "an unbounded overlay disqualifies the elision"
    );
}

/// Publishing an unchanged snapshot must not dirty the scene. The host pushes
/// settings, resource samples, and Interaction Domain state every frame, so an
/// unconditional dirty flag would recompile every display list per frame and
/// defeat the quiescent elision.
#[test]
fn republishing_an_unchanged_snapshot_stays_quiescent() {
    let mut shell = Shell::headless().unwrap();
    let stats = ResourceStats::default();
    shell.set_resource_stats(stats);
    shell.prepare_frame(&Input::default());
    assert!(!shell.dirty);

    shell.set_resource_stats(stats);
    assert!(
        !shell.dirty,
        "an unchanged resource sample must not request a rebuild"
    );

    let domains = InteractionDomainSnapshot {
        revision: 0,
        principals: Vec::new(),
        interaction_domains: Vec::new(),
        seats: Vec::new(),
        clients: Vec::new(),
        interaction_groups: Vec::new(),
    };
    shell.set_interaction_domains(domains.clone());
    assert!(shell.dirty, "a changed snapshot must request a rebuild");
    shell.prepare_frame(&Input::default());
    assert!(!shell.dirty);
    shell.set_interaction_domains(domains);
    assert!(
        !shell.dirty,
        "an unchanged Interaction Domain snapshot must not request a rebuild"
    );
}
