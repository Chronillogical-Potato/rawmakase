//! The Library's views, as Lightroom has them: Grid, Loupe, Compare and
//! Survey. Their buttons sit in the toolbar under the photos, so each view
//! is a click away with its key shown on hover; rating, flag and label keys
//! go where the view shows them.
use super::Library;
use crate::app::icons::{self, Icon};
use crate::app::shortcuts::keys_text;
use crate::app::theme;
use eframe::egui::{self, Vec2};

/// How the Library shows its photos.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum View {
    #[default]
    Grid,
    Loupe,
    Compare,
    Survey,
}
impl View {
    const ALL: [Self; 4] = [Self::Grid, Self::Loupe, Self::Compare, Self::Survey];
    fn icon(self) -> Icon {
        match self {
            Self::Grid => Icon::GridView,
            Self::Loupe => Icon::LoupeView,
            Self::Compare => Icon::BeforeAfter,
            Self::Survey => Icon::SurveyView,
        }
    }
    fn hover(self) -> String {
        let (name, key) = match self {
            Self::Grid => ("Grid", "G"),
            Self::Loupe => ("Loupe", "E"),
            Self::Compare => ("Compare", "C"),
            Self::Survey => ("Survey", "N"),
        };
        format!("{name} ({})", keys_text(key))
    }
}

impl Library {
    pub(super) fn view(&self) -> View {
        if self.compare.open {
            View::Compare
        } else if self.survey.open {
            View::Survey
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
            View::Survey => self.open_survey(),
        }
    }
    /// G: the grid, from any other view.
    pub fn show_grid(&mut self) {
        self.close_loupe();
        self.close_compare();
        self.close_survey();
    }
    /// Whether rating, flag and label keys go to the active photo alone, as
    /// in the Loupe, Compare and Survey, rather than to every photo selected.
    pub fn edits_active_only(&self) -> bool {
        self.view() != View::Grid
    }
    /// A rating, flag or label key, applied as the view shows photos: to
    /// every one selected in the grid, else to the active one. With
    /// `advance` (Shift), the view moves on to the next photo.
    pub fn edit_shown(
        &mut self,
        edit: crate::app::photo_metadata::Edit,
        advance: bool,
    ) -> anyhow::Result<()> {
        match (self.view(), self.selection.active) {
            (View::Compare, _) => self.edit_compared(edit, advance),
            (View::Survey, _) => self.edit_surveyed(edit, advance),
            (View::Loupe, Some(id)) => self.edit_metadata(id, edit, advance).map(drop),
            _ => self.edit_selection(edit, advance).map(drop),
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
