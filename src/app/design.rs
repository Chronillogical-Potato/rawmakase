//! The interface's design tokens: spacing, radii, type sizes, control
//! heights and the greys everything is drawn in. Change a value here and it
//! changes everywhere; drawing code names a token rather than a number.
//!
//! Colours go through [`Tone`]: a step on the default palette's grey ramp,
//! which [`theme::Palette::tone`](super::theme::Palette::tone) maps onto the
//! palette in use. Accent, danger and warning stay the palette's own.
use eframe::egui;

/// Gaps and padding, on a 4-point grid with 2 and 6 for tight spots.
pub(in crate::app) mod space {
    /// A hairline: a list row's padding, an underline's offset.
    pub(in crate::app) const HAIR: f32 = 1.;
    pub(in crate::app) const XXS: f32 = 2.;
    pub(in crate::app) const XS: f32 = 4.;
    pub(in crate::app) const S: f32 = 6.;
    pub(in crate::app) const M: f32 = 8.;
    pub(in crate::app) const L: f32 = 12.;
    pub(in crate::app) const XL: f32 = 16.;
    pub(in crate::app) const XXL: f32 = 24.;
    pub(in crate::app) const XXXL: f32 = 32.;
    /// Around an empty view's or the setup assistant's content.
    pub(in crate::app) const HUGE: f32 = 48.;
}

/// Insets shared by whole areas of the window.
pub(in crate::app) mod inset {
    use super::{margin, space};
    use eframe::egui::Margin;
    /// Around the photo, the Library's views and the status line, the same in
    /// every module so nothing shifts on a switch.
    pub(in crate::app) const STAGE: f32 = space::XL;
    /// A side panel's content from its edges.
    pub(in crate::app) const PANEL: f32 = space::M;
    /// A modal window's content from its border.
    pub(in crate::app) const DIALOG: f32 = space::XXL;
    /// A card's content (a group inside a dialog) from its border.
    pub(in crate::app) const CARD: Margin = margin(space::XL, space::L);
    /// A toolbar strip's content (the Library's views, its filter bar).
    pub(in crate::app) const BAR: Margin = margin(space::L, space::XS);
}

/// Corner radii.
pub(in crate::app) mod radius {
    /// Bars, tracks, swatches and small tags.
    pub(in crate::app) const SMALL: f32 = 2.;
    /// Buttons, fields, chips, list highlights.
    pub(in crate::app) const CONTROL: f32 = 4.;
    /// Cards, menus, overlays on the photo.
    pub(in crate::app) const CARD: f32 = 8.;
    /// Modal windows.
    pub(in crate::app) const DIALOG: f32 = 10.;
}

/// Type sizes, in points.
pub(in crate::app) mod text {
    /// Badges, counts, captions under thumbnails.
    pub(in crate::app) const MICRO: f32 = 10.;
    /// Panel captions and secondary rows: egui's Small.
    pub(in crate::app) const SMALL: f32 = 11.;
    /// List rows, values, dialog body text.
    pub(in crate::app) const LABEL: f32 = 12.;
    /// Buttons and default text: egui's Body.
    pub(in crate::app) const BODY: f32 = 13.;
    /// Emphasised lines and sub-headings.
    pub(in crate::app) const LARGE: f32 = 14.;
    /// A modal window's title.
    pub(in crate::app) const TITLE: f32 = 16.;
    /// The setup assistant's and Preferences' headings, drop hints.
    pub(in crate::app) const HERO: f32 = 18.;
    /// The setup assistant's first heading.
    pub(in crate::app) const DISPLAY: f32 = 26.;
}

/// Heights of rows and controls.
pub(in crate::app) mod size {
    /// A list row: folders, presets, metadata fields.
    pub(in crate::app) const ROW: f32 = 20.;
    /// A compact control: segments, small fields, panel switches.
    pub(in crate::app) const CONTROL: f32 = 22.;
    /// A row in a menu.
    pub(in crate::app) const MENU: f32 = 24.;
    /// A control in a dialog or form.
    pub(in crate::app) const FIELD: f32 = 26.;
    /// A tool button on Develop's toolbar.
    pub(in crate::app) const TOOL: f32 = 32.;
}

/// Padding inside buttons.
pub(in crate::app) mod button {
    use super::space;
    use eframe::egui::Vec2;
    /// The default button.
    pub(in crate::app) const PADDING: Vec2 = Vec2::new(space::M, space::S);
    /// A modal window's footer buttons ("Cancel", "Export").
    pub(in crate::app) const DIALOG: Vec2 = Vec2::new(space::XL, space::S);
    /// A button in a dense panel row.
    pub(in crate::app) const COMPACT: Vec2 = Vec2::new(space::S, space::XXS);
}

/// A margin of `x` at the sides and `y` at the top and bottom, from tokens.
pub(in crate::app) const fn margin(x: f32, y: f32) -> egui::Margin {
    egui::Margin::symmetric(x as i8, y as i8)
}
/// The same margin all round, from a token.
pub(in crate::app) const fn margin_same(v: f32) -> egui::Margin {
    egui::Margin::same(v as i8)
}

/// A grey of the interface, named by what it is for. Its level is the grey
/// in the default palette; other palettes map it between their own colours.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) struct Tone(u8);

impl Tone {
    // Surfaces, darkest first.
    /// Behind everything: the window's clear colour, thumbnail outlines.
    pub(in crate::app) const DEEP: Tone = Tone(15);
    /// Recessed wells: the histogram's and curve's backgrounds.
    pub(in crate::app) const WELL: Tone = Tone(20);
    /// The window and the side panels.
    pub(in crate::app) const WINDOW: Tone = Tone(22);
    /// The workspace bar, filmstrip and other strips along the window.
    pub(in crate::app) const BAR: Tone = Tone(26);
    /// A card or group set on a panel or dialog.
    pub(in crate::app) const CARD: Tone = Tone(32);
    /// Dialogs, pop-ups and the photo's backdrop.
    pub(in crate::app) const PANEL: Tone = Tone(35);
    /// Text fields, toolbars and the Library's grid.
    pub(in crate::app) const FIELD: Tone = Tone(38);
    /// Buttons, chips, key caps and a selected list row.
    pub(in crate::app) const RAISED: Tone = Tone(43);
    /// Borders, dividers, slider tracks and progress bars.
    pub(in crate::app) const OUTLINE: Tone = Tone(52);
    /// A control under the pointer.
    pub(in crate::app) const HOVER: Tone = Tone(59);
    /// A control being pressed or switched on.
    pub(in crate::app) const PRESSED: Tone = Tone(67);
    /// A border that has to stand out: focus, an on switch's track.
    pub(in crate::app) const OUTLINE_STRONG: Tone = Tone(82);
    // Text and icons, faintest first.
    /// Disabled text and icons.
    pub(in crate::app) const DISABLED: Tone = Tone(110);
    /// Placeholders, counts and hints.
    pub(in crate::app) const FAINT: Tone = Tone(125);
    /// Captions and secondary details.
    pub(in crate::app) const MUTED: Tone = Tone(140);
    /// Labels beside values, section titles.
    pub(in crate::app) const SECONDARY: Tone = Tone(150);
    /// Icons and labels a step above secondary.
    pub(in crate::app) const LABEL: Tone = Tone(170);
    /// Values and body text that is not the focus.
    pub(in crate::app) const SOFT: Tone = Tone(200);
    /// Text.
    pub(in crate::app) const TEXT: Tone = Tone(225);
    /// Titles, values being edited, hovered text.
    pub(in crate::app) const STRONG: Tone = Tone(236);
    /// The brightest marks: text on the accent, handles on the photo.
    pub(in crate::app) const BRIGHT: Tone = Tone(250);

    /// The tone's grey in the default palette.
    pub(in crate::app) const fn level(self) -> u8 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spacing_is_on_the_grid() {
        for v in [
            space::XS,
            space::M,
            space::L,
            space::XL,
            space::XXL,
            space::XXXL,
            space::HUGE,
        ] {
            assert_eq!(v % 4., 0., "{v} is off the 4-point grid");
        }
    }

    #[test]
    fn tones_get_lighter_in_order() {
        let ramp = [
            Tone::DEEP,
            Tone::WELL,
            Tone::WINDOW,
            Tone::BAR,
            Tone::CARD,
            Tone::PANEL,
            Tone::FIELD,
            Tone::RAISED,
            Tone::OUTLINE,
            Tone::HOVER,
            Tone::PRESSED,
            Tone::OUTLINE_STRONG,
            Tone::DISABLED,
            Tone::FAINT,
            Tone::MUTED,
            Tone::SECONDARY,
            Tone::LABEL,
            Tone::SOFT,
            Tone::TEXT,
            Tone::STRONG,
            Tone::BRIGHT,
        ];
        for pair in ramp.windows(2) {
            assert!(pair[0].level() < pair[1].level(), "{pair:?}");
        }
    }

    #[test]
    fn margins_keep_their_tokens() {
        assert_eq!(margin(space::XL, space::L), egui::Margin::symmetric(16, 12));
        assert_eq!(margin_same(inset::DIALOG), egui::Margin::same(24));
        assert_eq!(inset::CARD, egui::Margin::symmetric(16, 12));
    }
}
