//! The view buttons Lightroom puts in the toolbar under the photos: Grid,
//! Loupe and Compare, so each view is a click away and its key is shown on
//! hover.
use super::Library;
use crate::app::icons::{self, Icon};
use crate::app::shortcuts::keys_text;
use crate::app::theme;
use eframe::egui::{self, Vec2};

/// How the Library shows its photos.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum View {
    Grid,
    Loupe,
    Compare,
}
impl View {
    const ALL: [Self; 3] = [Self::Grid, Self::Loupe, Self::Compare];
    fn icon(self) -> Icon {
        match self {
            Self::Grid => Icon::GridView,
            Self::Loupe => Icon::LoupeView,
            Self::Compare => Icon::BeforeAfter,
        }
    }
    fn hover(self) -> String {
        let (name, key) = match self {
            Self::Grid => ("Grid", "G"),
            Self::Loupe => ("Loupe", "E"),
            Self::Compare => ("Compare", "C"),
        };
        format!("{name} ({})", keys_text(key))
    }
}

impl Library {
    pub(super) fn view(&self) -> View {
        if self.compare.open {
            View::Compare
        } else if self.loupe.open {
            View::Loupe
        } else {
            View::Grid
        }
    }
    pub(super) fn set_view(&mut self, view: View) {
        match view {
            View::Grid => self.show_grid(),
            View::Loupe => self.open_loupe(),
            View::Compare => self.open_compare(),
        }
    }
    /// The view buttons, the current one lit.
    pub(super) fn view_buttons(&mut self, ui: &mut egui::Ui) {
        let current = self.view();
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.x = 2.;
            for view in View::ALL {
                let (rect, response) =
                    ui.allocate_exact_size(Vec2::new(30., 22.), egui::Sense::click());
                let on = view == current;
                if on || response.hovered() {
                    let fill = theme::gray(if on { 78 } else { 55 });
                    ui.painter().rect_filled(rect, 3., fill);
                }
                let tint = theme::gray(if on { 240 } else { 165 });
                icons::paint_at(ui.painter(), view.icon(), rect.center(), 15., tint);
                if response.on_hover_text(view.hover()).clicked() && !on {
                    self.set_view(view);
                }
            }
        });
    }
}
