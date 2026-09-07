use lens::Frame;
use tessera_design::Design;
use tessera_i18n::{Localizer, Message};

pub(crate) fn unavailable_row(frame: &mut Frame, label: &str, i18n: &Localizer, design: &Design) {
    tessera_ui::render_unavailable_row(frame, label, i18n.text(Message::Unavailable), design);
}

pub(crate) use tessera_ui::{section_heading_layout, settings_card_layout};
