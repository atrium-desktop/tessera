//! Trusted visual feedback for input applied by an Agent Interaction Domain.
//!
//! The physical user's XDG cursor remains untouched. This component projects
//! each applied operation onto the human's read-only mirror as a
//! semi-transparent mask plus a text label naming the external operation, so
//! the observer still sees the window content underneath.
//!
//! Pursuant to ADR-0150 and ADR-0152:
//! - The pointer sprite dynamically reflects the `wp_cursor_shape_device_v1`
//!   cursor shape requested on the Agent's seat from the dedicated `tessera-ai`
//!   XDG cursor theme, rendered with its distinct open-fork silhouette.
//! - Click operations display a lightweight geometric ripple pulse at the click
//!   coordinate.
//! - Keyboard operations display a transient Keycast HUD anchored to the
//!   bottom-right corner of the window mirror, rendering keycap badges for
//!   recent keystrokes with smooth fade-in and fade-out animations.
//! - If the target is not visible, a compact background-operation pill is drawn
//!   at the screen top edge instead.
//!
//! Directed Interaction Domain capture renders client surfaces directly and
//! therefore never includes this layer, preventing an Agent from steering
//! from its own feedback layer.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

#[cfg(feature = "agent-sprites")]
use include_dir::{Dir, include_dir};
use lens::{Align, Color, Frame, Input, LayoutOpts, Rect};
use tessera_authority::interaction_domain::InteractionDomainId;
use tessera_authority::interaction_domain::InteractionDomainSnapshot;
use tessera_authority::interaction_domain::InteractionDomainState;
use tessera_design::Design;
use tessera_design::materials::{chrome_place, surface_layout};
use tessera_desktop::window::Window;
use tessera_desktop::window::WindowId;
use tessera_desktop::workspace::WorkspaceSnapshot;
use tessera_primitives::Point;

use crate::component::SCREEN_CORNER_INSET;
use crate::component::{
    AgentActivity, AgentInputKind, Chrome, ChromeEvents, ChromeUpdate, Localizer, Message,
    ellipsize,
};

const HOLD_FOR: Duration = Duration::from_secs(4);
const FADE_FOR: Duration = Duration::from_secs(2);
const VISIBLE_FOR: Duration = Duration::from_secs(6);
const CLICK_PULSE_FOR: Duration = Duration::from_millis(450);
const KEYCAST_HOLD_FOR: Duration = Duration::from_millis(1200);
const KEYCAST_FADE_FOR: Duration = Duration::from_millis(400);
const KEYCAST_VISIBLE_FOR: Duration = Duration::from_millis(1600);
const KEYCAST_HEIGHT: f32 = 28.0;
const MAX_KEYCAST_ITEMS: usize = 4;
const LABEL_HEIGHT: f32 = 28.0;
const BACKGROUND_HEIGHT: f32 = 34.0;
const CURSOR_SIZE: f32 = 24.0;

#[cfg(feature = "agent-sprites")]
static AI_THEME_LIGHT: Dir<'static> =
    include_dir!("$CARGO_MANIFEST_DIR/../../assets/cursors/tessera-ai-light");

#[cfg(feature = "agent-sprites")]
static AI_THEME_DARK: Dir<'static> =
    include_dir!("$CARGO_MANIFEST_DIR/../../assets/cursors/tessera-ai-dark");

/// Where an applied Agent operation is projected for the human observer.
#[derive(Debug, Clone, Copy, PartialEq)]
enum OperationRegion {
    Window { rect: Rect, radius: f32 },
}

/// One uploaded Agent cursor: a texture plus its hotspot as a *fraction* of the
/// sprite, because the mirror draws it at `CURSOR_SIZE` logical pixels and the
/// theme's own scale varies.
#[derive(Clone)]
struct LoadedCursor {
    image: std::rc::Rc<flux::Image>,
    hotspot: (f32, f32),
}

struct AgentCursorSet {
    cursors: HashMap<u32, LoadedCursor>,
    default_cursor: LoadedCursor,
}

impl AgentCursorSet {
    #[cfg(feature = "agent-sprites")]
    fn upload(device: &flux::Device, theme: &Dir<'static>) -> Option<Self> {
        let load_cursor = |shape: u32| -> Option<LoadedCursor> {
            for candidate in tessera_primitives::cursor_shape::xdg_names(shape) {
                // The theme's own SVG is the source of truth: it carries both the
                // art and the `data-hotspot-*` metadata, so there is no separate
                // PNG bake to keep in sync. A theme that ships a PNG still wins,
                // because that is the artist's own pre-rendered sprite.
                let document = theme
                    .get_file(format!("cursors/{candidate}.svg"))
                    .and_then(|file| tessera_assets::VectorDocument::parse(file.contents()).ok());
                let sprite = theme.get_file(format!("cursors/{candidate}.png"));
                let (image, hotspot) = match (sprite, document.as_ref()) {
                    (Some(sprite), document) => {
                        let image = upload_sprite(device, sprite.contents(), 96)?;
                        (image, normalized_hotspot(document))
                    }
                    (None, Some(document)) => {
                        let image = upload_document_sprite(device, document, 96)?;
                        (image, normalized_hotspot(Some(document)))
                    }
                    (None, None) => continue,
                };
                return Some(LoadedCursor {
                    image: std::rc::Rc::new(image),
                    hotspot,
                });
            }
            None
        };

        let default_cursor = load_cursor(tessera_primitives::cursor_shape::DEFAULT_SHAPE)?;
        let mut cursors = HashMap::new();
        cursors.insert(
            tessera_primitives::cursor_shape::DEFAULT_SHAPE,
            default_cursor.clone(),
        );

        for shape in 2..=36 {
            if let Some(loaded) = load_cursor(shape) {
                cursors.insert(shape, loaded);
            }
        }

        Some(Self {
            cursors,
            default_cursor,
        })
    }

    fn get(&self, shape: u32) -> &LoadedCursor {
        self.cursors.get(&shape).unwrap_or(&self.default_cursor)
    }
}

/// The theme's hotspot as a fraction of the sprite, read from the document's
/// `data-hotspot-*` metadata. A theme that stamps none gets the top-left corner,
/// which is the XDG convention for "no hotspot data".
#[cfg(feature = "agent-sprites")]
fn normalized_hotspot(document: Option<&tessera_assets::VectorDocument>) -> (f32, f32) {
    document.map_or(
        (0.0, 0.0),
        tessera_assets::VectorDocument::normalized_hotspot,
    )
}

/// Compile a theme document into an uploaded sprite of `size`x`size` pixels.
///
/// `PaintMode::theme` keeps the AI theme's own two-tone silhouette, which is
/// what distinguishes the Agent's pointer from the user's; `currentColor`
/// resolves to white so a theme that leaves a shape unpainted is still visible
/// over the mirror's scrim.
#[cfg(feature = "agent-sprites")]
fn upload_document_sprite(
    device: &flux::Device,
    document: &tessera_assets::VectorDocument,
    size: u32,
) -> Option<flux::Image> {
    let raster = document.rasterize(size).ok()?;
    flux::Image::from_bytes(
        device,
        raster.width(),
        raster.height(),
        flux::Format::Bgra8Unorm,
        raster.as_bgra(),
    )
    .ok()
}

/// Non-interactive, compositor-owned projection of Agent input activity.
pub struct AgentFeedback {
    interaction_domains: InteractionDomainSnapshot,
    activity: BTreeMap<InteractionDomainId, VisualActivity>,
    design: Design,
    cursor_sets: Option<[AgentCursorSet; 2]>,
}

#[derive(Debug, Clone)]
struct KeycastEntry {
    label: String,
    at: Instant,
}

#[derive(Debug, Clone)]
struct VisualActivity {
    latest: AgentActivity,
    latest_at: Instant,
    pointer_window: Option<WindowId>,
    pointer_position: Option<Point>,
    cursor_shape: Option<u32>,
    click_pulse: Option<ClickPulse>,
    keycast: Vec<KeycastEntry>,
}

#[derive(Debug, Clone, Copy)]
struct ClickPulse {
    position: Point,
    at: Instant,
}

impl AgentFeedback {
    /// Construct the feedback layer, uploading the pointer sprites through
    /// the composition root's flux device.
    #[must_use]
    pub fn new(device: &flux::Device) -> Self {
        #[cfg(feature = "agent-sprites")]
        let sets = AgentCursorSet::upload(device, &AI_THEME_LIGHT)
            .zip(AgentCursorSet::upload(device, &AI_THEME_DARK))
            .map(|(light, dark)| [light, dark]);
        #[cfg(not(feature = "agent-sprites"))]
        let sets = {
            // Sprites are an asset, not a requirement: the layer draws its
            // activity and keycast marks without them.
            let _ = device;
            None
        };
        Self::with_cursor_sets(sets)
    }

    fn with_cursor_sets(cursor_sets: Option<[AgentCursorSet; 2]>) -> Self {
        Self {
            interaction_domains:
                tessera_authority::interaction_domain::InteractionDomainModel::new().snapshot(),
            activity: BTreeMap::new(),
            design: Design::dark(),
            cursor_sets,
        }
    }

    #[cfg(test)]
    fn without_sprites() -> Self {
        Self::with_cursor_sets(None)
    }

    #[cfg(test)]
    fn update_agent_activity(&mut self, activity: &AgentActivity) {
        <Self as Chrome>::update(self, ChromeUpdate::AgentActivity(activity));
    }
}

impl Chrome for AgentFeedback {
    fn render(
        &mut self,
        f: &mut Frame,
        input: &Input,
        windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
        i18n: &Localizer,
        _out: &mut ChromeEvents,
    ) {
        let now = Instant::now();
        let live_interaction_domains = self
            .interaction_domains
            .interaction_domains
            .iter()
            .filter(|interaction_domain| {
                interaction_domain.state != InteractionDomainState::Revoked
            })
            .map(|interaction_domain| interaction_domain.id)
            .collect::<std::collections::BTreeSet<_>>();
        self.activity.retain(|interaction_domain, activity| {
            live_interaction_domains.contains(interaction_domain)
                && now.saturating_duration_since(activity.latest_at) < VISIBLE_FOR
        });

        let raw = input.as_raw();
        let display = (raw.display_size.x.max(1.0), raw.display_size.y.max(1.0));
        let interaction_domains = &self.interaction_domains;
        let design = &self.design;
        // Contrast polarity follows shell foreground: light on dark chrome,
        // dark on light chrome.
        let cursor_set = self.cursor_sets.as_ref().map(|variants| {
            &variants[usize::from(design.scheme == tessera_desktop::settings::ColorScheme::Light)]
        });
        let mut background = Vec::new();
        for (interaction_domain, activity) in &mut self.activity {
            let interaction_domain_state = interaction_domains
                .interaction_domains
                .iter()
                .find(|candidate| candidate.id == *interaction_domain)
                .map(|candidate| candidate.state)
                .unwrap_or(InteractionDomainState::Revoked);
            let age = now.saturating_duration_since(activity.latest_at);
            let alpha = activity_alpha(age, interaction_domain_state);

            // Clean expired keycast items
            activity
                .keycast
                .retain(|k| now.saturating_duration_since(k.at) < KEYCAST_VISIBLE_FOR);

            let projected = activity
                .pointer_window
                .zip(activity.pointer_position)
                .filter(|(_, position)| point_in_display(*position, display))
                .and_then(|(window, position)| {
                    operation_region(windows, window, position, display)
                        .map(|region| (region, Some(position)))
                })
                .or_else(|| {
                    activity
                        .pointer_window
                        .and_then(|window| window_mirror_region(windows, window, display))
                        .map(|region| (region, None))
                });

            if let Some((region, position)) = projected {
                render_pointer_feedback(
                    f,
                    *interaction_domain,
                    activity,
                    region,
                    position,
                    display,
                    interaction_domain_state,
                    alpha,
                    now,
                    i18n,
                    design,
                    cursor_set,
                );
            } else {
                background.push((
                    *interaction_domain,
                    activity.clone(),
                    interaction_domain_state,
                    alpha,
                ));
            }
        }

        for (index, (interaction_domain, activity, interaction_domain_state, alpha)) in
            background.into_iter().enumerate()
        {
            render_background_activity(
                f,
                interaction_domain,
                &activity,
                interaction_domain_state,
                alpha,
                display,
                index,
                i18n,
                design,
            );
        }
    }

    fn requires_composition(&self) -> bool {
        let now = Instant::now();
        self.activity.iter().any(|(interaction_domain, activity)| {
            now.saturating_duration_since(activity.latest_at) < VISIBLE_FOR
                && self
                    .interaction_domains
                    .interaction_domains
                    .iter()
                    .any(|candidate| {
                        candidate.id == *interaction_domain
                            && candidate.state != InteractionDomainState::Revoked
                    })
        })
    }

    fn update(&mut self, update: ChromeUpdate<'_>) {
        match update {
            ChromeUpdate::InteractionDomains(snapshot) => {
                self.interaction_domains = snapshot.clone();
                self.activity.retain(|interaction_domain, _| {
                    snapshot.interaction_domains.iter().any(|candidate| {
                        candidate.id == *interaction_domain
                            && candidate.state != InteractionDomainState::Revoked
                    })
                });
            }
            ChromeUpdate::Appearance(design) => self.design = *design,
            ChromeUpdate::AgentActivity(activity) => {
                let now = Instant::now();
                match self.activity.get_mut(&activity.interaction_domain) {
                    Some(state) => {
                        if activity.sequence <= state.latest.sequence {
                            return;
                        }
                        if let Some(position) = activity.position {
                            state.pointer_window = Some(activity.window);
                            state.pointer_position = Some(position);
                            if matches!(activity.kind, AgentInputKind::Click { .. })
                                || matches!(
                                    activity.kind,
                                    AgentInputKind::PointerButton {
                                        state: tessera_primitives::input::ButtonState::Pressed,
                                        ..
                                    }
                                )
                            {
                                state.click_pulse = Some(ClickPulse { position, at: now });
                            }
                        } else if state.pointer_window != Some(activity.window) {
                            state.pointer_window = Some(activity.window);
                        }
                        if let AgentInputKind::Keyboard {
                            key_name: Some(ref key),
                        } = activity.kind
                        {
                            state.keycast.push(KeycastEntry {
                                label: key.clone(),
                                at: now,
                            });
                            if state.keycast.len() > MAX_KEYCAST_ITEMS {
                                state.keycast.remove(0);
                            }
                        } else if let AgentInputKind::Key {
                            ref key_name,
                            state: tessera_primitives::input::ButtonState::Pressed,
                        } = activity.kind
                        {
                            state.keycast.push(KeycastEntry {
                                label: key_name.clone(),
                                at: now,
                            });
                            if state.keycast.len() > MAX_KEYCAST_ITEMS {
                                state.keycast.remove(0);
                            }
                        }
                        if activity.cursor_shape.is_some() {
                            state.cursor_shape = activity.cursor_shape;
                        }
                        state.latest = activity.clone();
                        state.latest_at = now;
                    }
                    None => {
                        let keycast = match activity.kind {
                            AgentInputKind::Keyboard {
                                key_name: Some(ref key),
                            } => vec![KeycastEntry {
                                label: key.clone(),
                                at: now,
                            }],
                            AgentInputKind::Key {
                                ref key_name,
                                state: tessera_primitives::input::ButtonState::Pressed,
                            } => vec![KeycastEntry {
                                label: key_name.clone(),
                                at: now,
                            }],
                            _ => Vec::new(),
                        };
                        let is_click = matches!(activity.kind, AgentInputKind::Click { .. })
                            || matches!(
                                activity.kind,
                                AgentInputKind::PointerButton {
                                    state: tessera_primitives::input::ButtonState::Pressed,
                                    ..
                                }
                            );
                        self.activity.insert(
                            activity.interaction_domain,
                            VisualActivity {
                                latest: activity.clone(),
                                latest_at: now,
                                pointer_window: Some(activity.window),
                                pointer_position: activity.position,
                                cursor_shape: activity.cursor_shape,
                                click_pulse: activity.position.and_then(|position| {
                                    if is_click {
                                        Some(ClickPulse { position, at: now })
                                    } else {
                                        None
                                    }
                                }),
                                keycast,
                            },
                        );
                    }
                }
            }
            _ => {}
        }
    }

    fn anim_pending(&self) -> bool {
        let now = Instant::now();
        self.activity.values().any(|activity| {
            now.saturating_duration_since(activity.latest_at) < VISIBLE_FOR
                || activity
                    .click_pulse
                    .is_some_and(|p| now.saturating_duration_since(p.at) < CLICK_PULSE_FOR)
                || activity
                    .keycast
                    .iter()
                    .any(|k| now.saturating_duration_since(k.at) < KEYCAST_VISIBLE_FOR)
        })
    }

    fn damage_regions(&self, windows: &[Window], display: (f32, f32)) -> Option<Vec<Rect>> {
        let now = Instant::now();
        let display = (display.0.max(1.0), display.1.max(1.0));
        // Each annotated window is its own region: two pointer operations on
        // distant windows must not become one box spanning both.
        let mut regions: Vec<Rect> = Vec::new();
        let mut background_count = 0usize;
        for (id, activity) in &self.activity {
            let expired = now.saturating_duration_since(activity.latest_at) >= VISIBLE_FOR;
            let revoked = !self
                .interaction_domains
                .interaction_domains
                .iter()
                .any(|candidate| {
                    candidate.id == *id && candidate.state != InteractionDomainState::Revoked
                });
            if expired || revoked {
                continue;
            }
            let projected_window = activity
                .pointer_window
                .zip(activity.pointer_position)
                .filter(|(_, position)| point_in_display(*position, display))
                .and_then(|(window, position)| {
                    operation_region(windows, window, position, display)
                        .map(|operation| (operation, Some(position)))
                })
                .or_else(|| {
                    activity
                        .pointer_window
                        .and_then(|window| window_mirror_region(windows, window, display))
                        .map(|operation| (operation, None))
                });

            match projected_window {
                Some((operation, position)) => {
                    let OperationRegion::Window { rect, .. } = operation;
                    let mask = tessera_primitives::Rect::new(
                        rect.x as i32,
                        rect.y as i32,
                        rect.w as i32,
                        rect.h as i32,
                    );
                    let label = match position {
                        Some(pos) => pointer_label_footprint(pos, display),
                        None => tessera_primitives::Rect::new(
                            rect.x as i32,
                            rect.y as i32,
                            rect.w as i32,
                            (LABEL_HEIGHT + 24.0) as i32,
                        ),
                    };
                    let ripple = match position {
                        Some(pos) => tessera_primitives::Rect::new(
                            (pos.x - 24).max(0),
                            (pos.y - 24).max(0),
                            48,
                            48,
                        ),
                        None => tessera_primitives::Rect::new(0, 0, 0, 0),
                    };
                    let keycast_rect = tessera_primitives::Rect::new(
                        (rect.x + rect.w - 240.0).max(0.0) as i32,
                        (rect.y + rect.h - 50.0).max(0.0) as i32,
                        240,
                        50,
                    );
                    for rect in [mask, label, ripple, keycast_rect] {
                        if rect.is_empty() {
                            continue;
                        }
                        let as_lens = Rect {
                            x: rect.origin.x as f32,
                            y: rect.origin.y as f32,
                            w: rect.size.w as f32,
                            h: rect.size.h as f32,
                        };
                        if !regions.contains(&as_lens) {
                            regions.push(as_lens);
                        }
                    }
                }
                None => background_count += 1,
            }
        }
        if background_count > 0 {
            let band = tessera_primitives::Rect::new(
                0,
                0,
                display.0 as i32,
                (BACKGROUND_HEIGHT * background_count as f32).ceil() as i32,
            );
            regions.push(Rect {
                x: band.origin.x as f32,
                y: band.origin.y as f32,
                w: band.size.w as f32,
                h: band.size.h as f32,
            });
        }
        Some(regions)
    }
}

fn window_mirror_region(
    windows: &[Window],
    window: WindowId,
    display: (f32, f32),
) -> Option<OperationRegion> {
    let window = windows
        .iter()
        .find(|candidate| candidate.id == window && candidate.read_only && !candidate.minimized)?;
    let left = (window.position.x as f32).max(0.0);
    let top = (window.position.y as f32).max(0.0);
    let right = (window.position.x as f32 + window.size.w as f32).min(display.0);
    let bottom = (window.position.y as f32 + window.size.h as f32).min(display.1);
    (right > left && bottom > top).then_some(OperationRegion::Window {
        rect: Rect {
            x: left,
            y: top,
            w: right - left,
            h: bottom - top,
        },
        radius: if window.state.fullscreen { 0.0 } else { 7.0 },
    })
}

fn operation_region(
    windows: &[Window],
    window: WindowId,
    position: Point,
    display: (f32, f32),
) -> Option<OperationRegion> {
    let window = windows.iter().find(|candidate| {
        candidate.id == window
            && candidate.read_only
            && !candidate.minimized
            && window_contains(candidate, position)
    })?;
    let left = (window.position.x as f32).max(0.0);
    let top = (window.position.y as f32).max(0.0);
    let right = (window.position.x as f32 + window.size.w as f32).min(display.0);
    let bottom = (window.position.y as f32 + window.size.h as f32).min(display.1);
    (right > left && bottom > top).then_some(OperationRegion::Window {
        rect: Rect {
            x: left,
            y: top,
            w: right - left,
            h: bottom - top,
        },
        radius: if window.state.fullscreen { 0.0 } else { 7.0 },
    })
}

#[allow(clippy::too_many_arguments)]
fn render_pointer_feedback(
    f: &mut Frame,
    interaction_domain: InteractionDomainId,
    activity: &VisualActivity,
    region: OperationRegion,
    position: Option<Point>,
    display: (f32, f32),
    interaction_domain_state: InteractionDomainState,
    alpha: u8,
    now: Instant,
    i18n: &Localizer,
    design: &Design,
    cursor_set: Option<&AgentCursorSet>,
) {
    if let Some(pos) = position {
        let shape = activity.cursor_shape.unwrap_or(1);
        let cursor = cursor_set.map(|set| set.get(shape));
        render_cursor(f, interaction_domain, cursor, pos, alpha);

        if let Some(pulse) = activity.click_pulse
            && now.saturating_duration_since(pulse.at) < CLICK_PULSE_FOR
        {
            render_click_ripple(
                f,
                interaction_domain,
                pulse.position,
                pulse.at,
                now,
                alpha,
                design,
            );
        }
    }

    let OperationRegion::Window { rect, radius } = region;
    render_shape(
        f,
        &format!("tessera-agent-mask-{}", interaction_domain.0),
        rect,
        design
            .colors
            .modal_scrim
            .with_alpha(scaled_alpha(alpha, 6, 17)),
        design
            .colors
            .application_border
            .with_alpha(scaled_alpha(alpha, 2, 3)),
        1.0,
        radius,
    );

    // Render Keycast HUD at the bottom-right of the operated window
    if !activity.keycast.is_empty() {
        render_keycast_hud(
            f,
            interaction_domain,
            rect,
            &activity.keycast,
            now,
            alpha,
            design,
        );
    }

    let label = activity_label(
        &activity.latest,
        interaction_domain_state,
        i18n,
        position.is_some(),
    );
    let measured = f.measure_text(&label, design.typography.footnote).width;
    let width = (measured + 20.0)
        .clamp(128.0, 290.0)
        .min((display.0 - 16.0).max(1.0));
    let label_rect = match position {
        Some(pos) => pointer_label_rect(pos, width, display),
        None => Rect {
            x: ((rect.x + rect.w * 0.5) - width * 0.5).max(rect.x + 8.0),
            y: (rect.y + 12.0).max(SCREEN_CORNER_INSET),
            w: width,
            h: LABEL_HEIGHT,
        },
    };
    let label = ellipsize(
        f,
        &label,
        design.typography.footnote,
        (label_rect.w - 14.0).max(0.0),
    );
    f.place(
        &format!("tessera-agent-label-{}", interaction_domain.0),
        &chrome_place(
            label_rect,
            LayoutOpts {
                bg: design
                    .colors
                    .application_surface
                    .with_alpha(scaled_alpha(alpha, 9, 10)),
                border: design
                    .colors
                    .application_border
                    .with_alpha(scaled_alpha(alpha, 3, 4)),
                border_width: 1.0,
                radius: LABEL_HEIGHT * 0.5,
                ..surface_layout()
            },
        ),
        |f| {
            f.centered(label_rect.w, label_rect.h, |f| {
                f.label_compact_sized(&label, design.typography.footnote)
            });
        },
    );
}

fn render_keycast_hud(
    f: &mut Frame,
    interaction_domain: InteractionDomainId,
    window_rect: Rect,
    keycast: &[KeycastEntry],
    now: Instant,
    alpha: u8,
    design: &Design,
) {
    let live_keys: Vec<_> = keycast
        .iter()
        .filter(|k| now.saturating_duration_since(k.at) < KEYCAST_VISIBLE_FOR)
        .collect();
    if live_keys.is_empty() {
        return;
    }

    let mut badge_widths = Vec::with_capacity(live_keys.len());
    let mut total_width = 0.0;
    for key in &live_keys {
        let measured = f.measure_text(&key.label, design.typography.footnote).width;
        let w = (measured + 18.0).max(28.0);
        badge_widths.push(w);
        total_width += w;
    }
    let gap = 6.0;
    total_width += gap * (live_keys.len().saturating_sub(1) as f32);

    let max_start_x = window_rect.x + window_rect.w - 16.0 - total_width;
    let mut cur_x = max_start_x.max(window_rect.x + 8.0);
    let y = (window_rect.y + window_rect.h - 16.0 - KEYCAST_HEIGHT).max(window_rect.y + 8.0);

    for (index, (key, &width)) in live_keys.iter().zip(&badge_widths).enumerate() {
        let age = now.saturating_duration_since(key.at);
        let key_alpha = if age <= KEYCAST_HOLD_FOR {
            alpha
        } else {
            let fade =
                age.saturating_sub(KEYCAST_HOLD_FOR).as_secs_f32() / KEYCAST_FADE_FOR.as_secs_f32();
            ((alpha as f32) * (1.0 - fade.clamp(0.0, 1.0))).round() as u8
        };

        let badge_rect = Rect {
            x: cur_x,
            y,
            w: width,
            h: KEYCAST_HEIGHT,
        };
        cur_x += width + gap;

        f.place(
            &format!("tessera-agent-keycast-{}-{index}", interaction_domain.0),
            &chrome_place(
                badge_rect,
                LayoutOpts {
                    bg: design
                        .colors
                        .application_surface
                        .with_alpha(scaled_alpha(key_alpha, 9, 10)),
                    border: design
                        .colors
                        .application_border
                        .with_alpha(scaled_alpha(key_alpha, 4, 5)),
                    border_width: 1.0,
                    radius: 6.0,
                    ..surface_layout()
                },
            ),
            |f| {
                f.centered(badge_rect.w, badge_rect.h, |f| {
                    f.label_compact_sized(&key.label, design.typography.footnote)
                });
            },
        );
    }
}

fn render_cursor(
    f: &mut Frame,
    interaction_domain: InteractionDomainId,
    cursor: Option<&LoadedCursor>,
    position: Point,
    alpha: u8,
) {
    let id = format!("tessera-agent-glyph-{}", interaction_domain.0);
    match cursor {
        Some(cursor) => {
            let rect = Rect {
                x: position.x as f32 - CURSOR_SIZE * cursor.hotspot.0,
                y: position.y as f32 - CURSOR_SIZE * cursor.hotspot.1,
                w: CURSOR_SIZE,
                h: CURSOR_SIZE,
            };
            f.place(
                &id,
                &chrome_place(rect, LayoutOpts::default()),
                |f| unsafe {
                    f.image_tinted(
                        cursor.image.as_raw(),
                        rect.w,
                        rect.h,
                        Color::rgba(255, 255, 255, alpha),
                    );
                },
            );
        }
        None => {
            render_shape(
                f,
                &id,
                centered_rect(position, 8.0),
                Color::rgba(244, 246, 252, alpha),
                Color::TRANSPARENT,
                0.0,
                4.0,
            );
        }
    }
}

fn render_click_ripple(
    f: &mut Frame,
    interaction_domain: InteractionDomainId,
    position: Point,
    pulse_at: Instant,
    now: Instant,
    alpha: u8,
    design: &Design,
) {
    let elapsed = now.saturating_duration_since(pulse_at).as_secs_f32();
    let total = CLICK_PULSE_FOR.as_secs_f32();
    let t = (elapsed / total).clamp(0.0, 1.0);
    let radius = 6.0 + 16.0 * t;
    let ripple_alpha = ((1.0 - t) * (alpha as f32 / 255.0) * 200.0) as u8;
    let rect = Rect {
        x: position.x as f32 - radius,
        y: position.y as f32 - radius,
        w: radius * 2.0,
        h: radius * 2.0,
    };
    render_shape(
        f,
        &format!("tessera-agent-click-ripple-{}", interaction_domain.0),
        rect,
        Color::TRANSPARENT,
        design.colors.application_accent.with_alpha(ripple_alpha),
        1.5,
        radius,
    );
}

#[allow(clippy::too_many_arguments)]
fn render_background_activity(
    f: &mut Frame,
    interaction_domain: InteractionDomainId,
    activity: &VisualActivity,
    interaction_domain_state: InteractionDomainState,
    alpha: u8,
    display: (f32, f32),
    index: usize,
    i18n: &Localizer,
    design: &Design,
) {
    let label = activity_label(&activity.latest, interaction_domain_state, i18n, false);
    let measured = f.measure_text(&label, design.typography.footnote).width;
    let width = (measured + 28.0)
        .clamp(190.0, 360.0)
        .min((display.0 - 16.0).max(1.0));
    let rect = Rect {
        x: ((display.0 - width) * 0.5).max(8.0),
        y: SCREEN_CORNER_INSET + index as f32 * (BACKGROUND_HEIGHT + 7.0),
        w: width,
        h: BACKGROUND_HEIGHT,
    };
    let label = ellipsize(
        f,
        &label,
        design.typography.footnote,
        (rect.w - 31.0).max(0.0),
    );
    let border = design
        .colors
        .application_border
        .with_alpha(scaled_alpha(alpha, 1, 2));
    f.place(
        &format!("tessera-agent-background-{}", interaction_domain.0),
        &chrome_place(
            rect,
            LayoutOpts {
                bg: design
                    .colors
                    .application_surface
                    .with_alpha(scaled_alpha(alpha, 9, 10)),
                border,
                border_width: 1.0,
                radius: BACKGROUND_HEIGHT * 0.5,
                ..surface_layout()
            },
        ),
        |f| {
            f.row_ex(
                &LayoutOpts {
                    width: rect.w,
                    height: rect.h,
                    gap: 8.0,
                    pad: 8.0,
                    cross: Align::Center,
                    ..Default::default()
                },
                |f| {
                    f.column_ex(
                        &LayoutOpts {
                            width: 7.0,
                            height: 7.0,
                            bg: design.colors.menu_text_heading.with_alpha(alpha),
                            radius: 3.5,
                            ..Default::default()
                        },
                        |_| {},
                    );
                    f.label_compact_sized(&label, design.typography.footnote);
                },
            );
        },
    );
}

fn activity_label(
    activity: &AgentActivity,
    state: InteractionDomainState,
    i18n: &Localizer,
    pointer_visible: bool,
) -> String {
    let interaction_domain = &activity.interaction_domain_label;
    let operation = operation_label(&activity.kind, i18n);
    let state_suffix = if state == InteractionDomainState::Paused {
        format!(" · {}", i18n.text(Message::InteractionDomainPaused))
    } else {
        String::new()
    };
    if pointer_visible {
        format!(
            "{} · {interaction_domain} · {operation}{state_suffix}",
            i18n.text(Message::AgentBadge)
        )
    } else {
        format!(
            "{interaction_domain} · {} · {operation}{state_suffix}",
            i18n.text(Message::AgentOperating)
        )
    }
}

fn operation_label(kind: &AgentInputKind, i18n: &Localizer) -> String {
    match kind {
        AgentInputKind::PointerMove => i18n.text(Message::AgentPointerMove).to_owned(),
        AgentInputKind::Click { button: 0x111 } => i18n.text(Message::AgentRightClick).to_owned(),
        AgentInputKind::Click { button: 0x112 } => i18n.text(Message::AgentMiddleClick).to_owned(),
        AgentInputKind::Click { .. } => i18n.text(Message::AgentClick).to_owned(),
        AgentInputKind::Scroll { dx, dy } if dy.abs() >= dx.abs() && *dy < 0.0 => {
            i18n.text(Message::AgentScrollUp).to_owned()
        }
        AgentInputKind::Scroll { dx, dy } if dy.abs() >= dx.abs() => {
            i18n.text(Message::AgentScrollDown).to_owned()
        }
        AgentInputKind::Scroll { dx, .. } if *dx < 0.0 => {
            i18n.text(Message::AgentScrollLeft).to_owned()
        }
        AgentInputKind::Scroll { .. } => i18n.text(Message::AgentScrollRight).to_owned(),
        AgentInputKind::Keyboard {
            key_name: Some(key),
        } => format!("{}: {key}", i18n.text(Message::AgentKeyboard)),
        AgentInputKind::Keyboard { key_name: None } => i18n.text(Message::AgentKeyboard).to_owned(),
        AgentInputKind::PointerButton { button: 0x111, .. } => {
            i18n.text(Message::AgentRightClick).to_owned()
        }
        AgentInputKind::PointerButton { button: 0x112, .. } => {
            i18n.text(Message::AgentMiddleClick).to_owned()
        }
        AgentInputKind::PointerButton { .. } => i18n.text(Message::AgentClick).to_owned(),
        AgentInputKind::Key { key_name, .. } => {
            format!("{}: {key_name}", i18n.text(Message::AgentKeyboard))
        }
    }
}

fn activity_alpha(age: Duration, state: InteractionDomainState) -> u8 {
    let state_alpha = if state == InteractionDomainState::Paused {
        150.0
    } else {
        255.0
    };
    if age <= HOLD_FOR {
        return state_alpha as u8;
    }
    let fade = age.saturating_sub(HOLD_FOR).as_secs_f32() / FADE_FOR.as_secs_f32();
    (state_alpha * (1.0 - fade.clamp(0.0, 1.0))).round() as u8
}

fn scaled_alpha(alpha: u8, numerator: u16, denominator: u16) -> u8 {
    let scaled = u16::from(alpha).saturating_mul(numerator) / denominator.max(1);
    u8::try_from(scaled.min(255)).unwrap_or(255)
}

/// Decode one cursor sprite and upload it.
///
/// Absent the `agent-sprites` feature the sprites are simply unavailable, which
/// this module already models: `cursor_sets` is an `Option` and every consumer
/// degrades to no on-screen pointer. Compiling an asset decoder into a build that
/// does not want it is the thing to avoid, not the absence of the sprites.
#[cfg(feature = "agent-sprites")]
fn upload_sprite(device: &flux::Device, png: &[u8], size: u32) -> Option<flux::Image> {
    let raster = tessera_assets::decode_bytes(png, size).ok()?;
    flux::Image::from_bytes(
        device,
        raster.width(),
        raster.height(),
        flux::Format::Bgra8Unorm,
        raster.as_bgra(),
    )
    .ok()
}

fn window_contains(window: &Window, position: Point) -> bool {
    window.size.w > 0
        && window.size.h > 0
        && position.x >= window.position.x
        && position.y >= window.position.y
        && position.x < window.position.x.saturating_add(window.size.w)
        && position.y < window.position.y.saturating_add(window.size.h)
}

fn point_in_display(position: Point, display: (f32, f32)) -> bool {
    position.x >= 0
        && position.y >= 0
        && (position.x as f32) < display.0
        && (position.y as f32) < display.1
}

fn centered_rect(position: Point, diameter: f32) -> Rect {
    Rect {
        x: position.x as f32 - diameter * 0.5,
        y: position.y as f32 - diameter * 0.5,
        w: diameter,
        h: diameter,
    }
}

fn pointer_label_rect(position: Point, width: f32, display: (f32, f32)) -> Rect {
    let right = position.x as f32 + 20.0;
    let x = if right + width <= display.0 - 8.0 {
        right
    } else {
        (position.x as f32 - width - 20.0).max(8.0)
    };
    let below = position.y as f32 + 18.0;
    let y = if below + LABEL_HEIGHT <= display.1 - 8.0 {
        below.max(SCREEN_CORNER_INSET)
    } else {
        (position.y as f32 - LABEL_HEIGHT - 18.0).max(SCREEN_CORNER_INSET)
    };
    Rect {
        x,
        y,
        w: width,
        h: LABEL_HEIGHT,
    }
}

fn pointer_label_footprint(position: Point, display: (f32, f32)) -> tessera_primitives::Rect {
    let max_width = 290.0f32.min((display.0 - 16.0).max(1.0));
    let x0 = (position.x as f32 - max_width - 20.0)
        .min(position.x as f32 - CURSOR_SIZE * 0.5)
        .max(0.0);
    let x1 = (position.x as f32 + 20.0 + max_width)
        .max(position.x as f32 + CURSOR_SIZE)
        .min(display.0);
    let y0 = (position.y as f32 - LABEL_HEIGHT - 18.0).max(0.0);
    let y1 = (position.y as f32 + 18.0 + LABEL_HEIGHT).min(display.1);
    tessera_primitives::Rect::new(
        x0.floor() as i32,
        y0.floor() as i32,
        (x1 - x0).ceil() as i32,
        (y1 - y0).ceil() as i32,
    )
}

fn render_shape(
    f: &mut Frame,
    id: &str,
    rect: Rect,
    background: Color,
    border: Color,
    border_width: f32,
    radius: f32,
) {
    f.place(
        id,
        &chrome_place(
            rect,
            LayoutOpts {
                bg: background,
                border,
                border_width,
                radius,
                ..surface_layout()
            },
        ),
        |f| {
            f.column_ex(
                &LayoutOpts {
                    width: rect.w,
                    height: rect.h,
                    ..Default::default()
                },
                |_| {},
            );
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sprite pipeline's coverage invariant. It needs the decoder, so it is
    /// gated with the feature that provides it rather than forcing every build
    /// without sprites to compile an asset decoder for a test.
    #[cfg(feature = "agent-sprites")]
    #[test]
    fn ai_polarities_rasterize_with_identical_coverage() {
        let shapes = ["default", "pointer", "text", "crosshair", "wait"];
        for shape in shapes {
            let light_file = AI_THEME_LIGHT
                .get_file(format!("cursors/{shape}.png"))
                .expect("light shape exists");
            let dark_file = AI_THEME_DARK
                .get_file(format!("cursors/{shape}.png"))
                .expect("dark shape exists");

            let render = |png: &[u8]| {
                tessera_assets::decode_bytes(png, 24)
                    .expect("sprite decodes")
                    .into_bgra()
            };
            let a = render(light_file.contents());
            let b = render(dark_file.contents());
            assert_ne!(a, b);
            assert!(a.chunks_exact(4).any(|pixel| pixel[3] > 0));
            assert!(
                a.chunks_exact(4)
                    .zip(b.chunks_exact(4))
                    .all(|(a, b)| a[3] == b[3])
            );
        }
    }

    fn activity(
        sequence: u64,
        kind: AgentInputKind,
        position: Option<Point>,
        cursor_shape: Option<u32>,
    ) -> AgentActivity {
        AgentActivity {
            sequence,
            interaction_domain: InteractionDomainId(7),
            interaction_domain_label: "Fuji".into(),
            window: WindowId(42),
            position,
            kind,
            cursor_shape,
        }
    }

    #[test]
    fn keyboard_activity_manifests_key_label() {
        let mut feedback = AgentFeedback::without_sprites();
        feedback.update_agent_activity(&activity(
            1,
            AgentInputKind::PointerMove,
            Some(Point { x: 120, y: 80 }),
            Some(1),
        ));
        feedback.update_agent_activity(&activity(
            2,
            AgentInputKind::Keyboard {
                key_name: Some("↵ Enter".into()),
            },
            None,
            None,
        ));

        let visual = feedback
            .activity
            .get(&InteractionDomainId(7))
            .expect("activity");
        assert_eq!(visual.pointer_position, Some(Point { x: 120, y: 80 }));
        assert_eq!(visual.cursor_shape, Some(1));
        assert_eq!(
            visual.latest.kind,
            AgentInputKind::Keyboard {
                key_name: Some("↵ Enter".into())
            }
        );
        assert_eq!(visual.keycast.len(), 1);
        assert_eq!(visual.keycast[0].label, "↵ Enter");
        assert_eq!(
            operation_label(&visual.latest.kind, &Localizer::new("en-US")),
            "Keyboard: ↵ Enter"
        );
    }

    #[test]
    fn stale_activity_cannot_rewind_visual_state() {
        let mut feedback = AgentFeedback::without_sprites();
        feedback.update_agent_activity(&activity(
            2,
            AgentInputKind::Keyboard {
                key_name: Some("Esc".into()),
            },
            None,
            None,
        ));
        feedback.update_agent_activity(&activity(
            1,
            AgentInputKind::Click { button: 0x110 },
            Some(Point { x: 1, y: 2 }),
            Some(4),
        ));
        let visual = feedback
            .activity
            .get(&InteractionDomainId(7))
            .expect("activity");
        assert_eq!(visual.latest.sequence, 2);
        assert_eq!(visual.pointer_position, None);
    }

    #[test]
    fn region_projects_only_inside_a_read_only_human_mirror() {
        let mut window = Window::new(WindowId(42));
        window.position = Point { x: 20, y: 30 };
        window.size = tessera_primitives::Size { w: 100, h: 80 };
        assert!(window_contains(&window, Point { x: 25, y: 35 }));
        assert!(!window.read_only);
        window.read_only = true;
        assert!(window.read_only && window_contains(&window, Point { x: 25, y: 35 }));
        assert!(!window_contains(&window, Point { x: 120, y: 35 }));
    }

    #[test]
    fn click_pulse_is_recorded() {
        let position = Point { x: 50, y: 50 };
        let act = activity(
            1,
            AgentInputKind::Click { button: 0x110 },
            Some(position),
            Some(1),
        );
        let mut feedback = AgentFeedback::without_sprites();
        feedback.update_agent_activity(&act);
        let visual = feedback.activity.get(&InteractionDomainId(7)).unwrap();
        assert!(visual.click_pulse.is_some());
        assert_eq!(visual.click_pulse.unwrap().position, position);
    }

    #[test]
    fn cursor_shape_updates_dynamically() {
        let mut feedback = AgentFeedback::without_sprites();
        feedback.update_agent_activity(&activity(
            1,
            AgentInputKind::PointerMove,
            Some(Point { x: 10, y: 10 }),
            Some(1),
        ));
        assert_eq!(
            feedback
                .activity
                .get(&InteractionDomainId(7))
                .unwrap()
                .cursor_shape,
            Some(1)
        );
        feedback.update_agent_activity(&activity(
            2,
            AgentInputKind::PointerMove,
            Some(Point { x: 20, y: 20 }),
            Some(9), // text / ibeam
        ));
        assert_eq!(
            feedback
                .activity
                .get(&InteractionDomainId(7))
                .unwrap()
                .cursor_shape,
            Some(9)
        );
    }

    #[test]
    fn keycast_hud_accumulates_and_bounds_items() {
        let mut feedback = AgentFeedback::without_sprites();
        for i in 1..=6 {
            feedback.update_agent_activity(&activity(
                i,
                AgentInputKind::Keyboard {
                    key_name: Some(format!("Key{i}")),
                },
                None,
                None,
            ));
        }
        let visual = feedback.activity.get(&InteractionDomainId(7)).unwrap();
        assert_eq!(visual.keycast.len(), MAX_KEYCAST_ITEMS);
        assert_eq!(visual.keycast.last().unwrap().label, "Key6");
    }

    #[test]
    fn pointer_button_and_key_states_manifest_in_feedback() {
        let mut feedback = AgentFeedback::without_sprites();
        // Drag press down
        feedback.update_agent_activity(&activity(
            1,
            AgentInputKind::PointerButton {
                button: 0x110,
                state: tessera_primitives::input::ButtonState::Pressed,
            },
            Some(Point { x: 40, y: 50 }),
            Some(1),
        ));
        let visual = feedback.activity.get(&InteractionDomainId(7)).unwrap();
        assert!(visual.click_pulse.is_some());
        assert_eq!(visual.click_pulse.unwrap().position, Point { x: 40, y: 50 });

        // Key down
        feedback.update_agent_activity(&activity(
            2,
            AgentInputKind::Key {
                key_name: "Ctrl".into(),
                state: tessera_primitives::input::ButtonState::Pressed,
            },
            None,
            None,
        ));
        let visual = feedback.activity.get(&InteractionDomainId(7)).unwrap();
        assert_eq!(visual.keycast.last().unwrap().label, "Ctrl");
    }

    #[test]
    fn labels_are_localized_and_unicode_safe() {
        let zh = Localizer::new("zh-CN");
        assert_eq!(
            operation_label(
                &AgentInputKind::Keyboard {
                    key_name: Some("↵ 回车".into())
                },
                &zh
            ),
            "键盘输入: ↵ 回车"
        );
        assert_eq!(
            operation_label(&AgentInputKind::Click { button: 0x111 }, &zh),
            "右键点击"
        );
        assert_eq!(crate::component::truncate("智能体正在操作", 5), "智能体正…");
    }
}
