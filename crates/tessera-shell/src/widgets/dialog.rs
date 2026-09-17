//! Modal dialog scaffolding, standard action buttons, and keyboard traps.

use lens::{Frame, LayoutOpts, Rect};
use tessera_design::{Design, materials};

use crate::widgets::geom::contains;

/// Standard modal dialog width for Tessera prompts.
pub const DEFAULT_MODAL_WIDTH: f32 = 460.0;
/// Standard modal inner padding.
pub const DEFAULT_MODAL_PAD: f32 = 16.0;
/// Standard title row height.
pub const DEFAULT_TITLE_HEIGHT: f32 = 24.0;
/// Standard action button height.
pub const DEFAULT_BUTTON_HEIGHT: f32 = 30.0;
/// Standard action button width for binary yes/no dialogs.
pub const DEFAULT_BUTTON_WIDTH: f32 = 96.0;
/// Standard Gaussian blur sigma for modal backdrop regions.
pub const DEFAULT_BACKDROP_BLUR_SIGMA: f32 = 18.0;

/// Standard labels for the four ADR-0088 runtime grant persistence levels:
/// Deny, Allow once, This session, Always.
pub const GRANT_LABELS: [&str; 4] = ["Deny", "Allow once", "This session", "Always"];
/// The index of the default affirmative grant option ("Allow once").
pub const GRANT_ACCENT_INDEX: usize = 1;

/// Style variant for action buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ActionButtonStyle {
    /// Default surface background (unaccented).
    #[default]
    Default,
    /// Accented affirmative button (e.g. primary action or "Allow once").
    Accented,
    /// Subtle/card surface background for secondary actions (e.g. "Cancel").
    Subtle,
}

/// Place the modal's Liquid Glass background panel.
pub fn place_modal_panel(frame: &mut Frame, id: &str, panel_rect: Rect, design: &Design) {
    frame.place(
        id,
        &materials::chrome_place(panel_rect, materials::glass_panel(design)),
        |_| {},
    );
}

/// Render a single styled action button.
pub fn render_action_button(
    frame: &mut Frame,
    id: &str,
    rect: Rect,
    label: &str,
    style: ActionButtonStyle,
    is_hovered: bool,
    design: &Design,
) {
    let bg = match style {
        ActionButtonStyle::Accented => {
            if is_hovered {
                design.colors.application_surface_active
            } else {
                design.colors.application_accent
            }
        }
        ActionButtonStyle::Subtle | ActionButtonStyle::Default => {
            if is_hovered {
                design.colors.application_surface_hover
            } else {
                design.colors.card_surface
            }
        }
    };

    frame.place(
        id,
        &materials::chrome_place(
            rect,
            LayoutOpts {
                bg,
                radius: design.radii.control,
                pad: 0.0,
                ..materials::surface_layout()
            },
        ),
        |frame| {
            frame.centered(rect.w, rect.h, |frame| {
                frame.label_compact_sized(label, design.typography.body);
            });
        },
    );
}

/// Render the four standard ADR-0088 runtime grant buttons:
/// [Deny] [Allow once] [This session] [Always].
pub fn render_grant_action_buttons(
    frame: &mut Frame,
    base_id: &str,
    rects: &[Rect; 4],
    cursor: (f32, f32),
    design: &Design,
) {
    for (index, (&rect, &label)) in rects.iter().zip(GRANT_LABELS.iter()).enumerate() {
        let is_hovered = contains(rect, cursor.0, cursor.1);
        let style = if index == GRANT_ACCENT_INDEX {
            ActionButtonStyle::Accented
        } else {
            ActionButtonStyle::Subtle
        };
        render_action_button(
            frame,
            &format!("{base_id}-grant-{index}"),
            rect,
            label,
            style,
            is_hovered,
            design,
        );
    }
}
