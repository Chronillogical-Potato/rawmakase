//! The Tone Curve panel's Point Curve menu: Lightroom's Linear, Medium Contrast and
//! Strong Contrast, the curves saved here, and Save… to save the photo's whole point
//! curve under a name. Choosing a curve is one History step.
use super::widgets::{modal_frame, primary_button};
use super::{Editor, history::Step, theme};
use crate::presets::curves::{BuiltinCurve, PointCurve, SavedCurve, SavedCurves};
use eframe::egui::{self, Color32, Vec2};

/// The saved curves, read once when the menu is first shown, and the name being
/// typed in the Save Point Curve window while it is open.
#[derive(Default)]
pub(super) struct CurveMenu {
    saved: Option<Vec<SavedCurve>>,
    saving: Option<CurveSave>,
}

/// The Save Point Curve window's name field, which takes the keyboard once.
struct CurveSave {
    name: String,
    focused: bool,
}

/// What was chosen in the menu.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum CurveChoice {
    Builtin(BuiltinCurve),
    Saved(SavedCurve),
    Save,
}

/// What the Save Point Curve window asked for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SaveChoice {
    Save,
    Cancel,
}

impl Editor {
    /// The saved curves, read from the Curves folder the first time.
    pub(super) fn saved_curves(&mut self) -> Vec<SavedCurve> {
        if self.curves.saved.is_none() {
            self.reload_saved_curves(&SavedCurves::default());
        }
        self.curves.saved.clone().unwrap_or_default()
    }
    fn reload_saved_curves(&mut self, store: &SavedCurves) {
        let list = store.list();
        if let Some(error) = list.errors.first() {
            self.status = format!("Point curve not read: {error}");
        }
        self.curves.saved = Some(list.curves);
    }
    /// Carries out a choice from the Point Curve menu.
    pub(super) fn choose_point_curve(&mut self, choice: CurveChoice) {
        let r = &mut self.document.recipe;
        let name = match choice {
            CurveChoice::Builtin(curve) => {
                curve.apply(r);
                curve.name().to_string()
            }
            CurveChoice::Saved(saved) => {
                saved.curve.apply(r);
                saved.name
            }
            CurveChoice::Save => {
                self.curves.saving = Some(CurveSave {
                    name: String::new(),
                    focused: false,
                });
                return;
            }
        };
        self.document.history.label(Step::new("Point Curve", name));
    }
    /// Whether the Save Point Curve window is open.
    pub(super) fn curve_save_open(&self) -> bool {
        self.curves.saving.is_some()
    }
    /// The Save Point Curve window, while open.
    pub(super) fn curve_save_window(&mut self, ctx: &egui::Context) {
        let Some(saving) = &mut self.curves.saving else {
            return;
        };
        let mut choice = None;
        let response = egui::Modal::new(egui::Id::new("save-point-curve"))
            .backdrop_color(Color32::from_black_alpha(140))
            .frame(modal_frame().inner_margin(egui::Margin::symmetric(28, 22)))
            .show(ctx, |ui| {
                ui.set_width(380.);
                ui.label(
                    egui::RichText::new("Save Point Curve")
                        .size(17.)
                        .color(theme::gray(235)),
                );
                ui.add_space(6.);
                ui.label(
                    egui::RichText::new(
                        "Saves the RGB, Red, Green and Blue curves for the Point Curve menu.",
                    )
                    .size(11.)
                    .color(theme::gray(150)),
                );
                ui.add_space(12.);
                let field = ui.add(
                    egui::TextEdit::singleline(&mut saving.name)
                        .hint_text("Curve name")
                        .desired_width(f32::INFINITY),
                );
                if !saving.focused {
                    field.request_focus();
                    saving.focused = true;
                }
                let named = !saving.name.trim().is_empty();
                if named && field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    choice = Some(SaveChoice::Save);
                }
                ui.add_space(16.);
                ui.horizontal(|ui| {
                    ui.spacing_mut().button_padding = Vec2::new(14., 6.);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled_ui(named, |ui| primary_button(ui, "Save"))
                            .inner
                            .clicked()
                        {
                            choice = Some(SaveChoice::Save);
                        }
                        if ui.button("Cancel").clicked() {
                            choice = Some(SaveChoice::Cancel);
                        }
                    });
                });
            });
        if response.should_close() {
            choice = Some(SaveChoice::Cancel);
        }
        let Some(choice) = choice else {
            return;
        };
        let saving = self.curves.saving.take().expect("open above");
        if choice == SaveChoice::Save {
            self.save_point_curve(&SavedCurves::default(), &saving.name);
        }
    }
    /// Saves the photo's point curve as `name` in `store`, and lists it.
    fn save_point_curve(&mut self, store: &SavedCurves, name: &str) {
        let curve = PointCurve::of(&self.document.recipe);
        match store.save(name, &curve) {
            Ok(_) => {
                self.reload_saved_curves(store);
                self.status = format!("Point curve {} saved", name.trim());
            }
            Err(e) => self.status = format!("Point curve not saved: {e:#}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::develop::panels::{Panel, PanelState};

    #[test]
    fn choosing_a_curve_is_one_named_history_step_that_turns_the_panel_on() {
        let ctx = egui::Context::default();
        let mut e = Editor::with_context(&ctx, None, crate::storage::Session::default(), None);
        e.document
            .recipe
            .panels
            .set(Panel::ToneCurve, PanelState::Off);
        let before = e.document.recipe.clone();
        let frame = e.begin_edit_frame();
        e.choose_point_curve(CurveChoice::Builtin(BuiltinCurve::MediumContrast));
        e.finish_edit_frame(frame, &ctx);
        let r = &e.document.recipe;
        assert_eq!(r.curve, BuiltinCurve::MediumContrast.curve());
        assert_eq!(r.panels.state(Panel::ToneCurve), PanelState::On);
        let (steps, applied) = e.document.history.steps();
        assert_eq!(applied, 1);
        assert_eq!(
            (steps[0].name.as_str(), steps[0].value.as_str()),
            ("Point Curve", "Medium Contrast")
        );
        e.undo();
        assert_eq!(e.document.recipe, before);
    }

    #[test]
    fn a_saved_curve_is_listed_and_loads_all_four_curves_in_one_step() {
        let ctx = egui::Context::default();
        let mut e = Editor::with_context(&ctx, None, crate::storage::Session::default(), None);
        let d = tempfile::tempdir().unwrap();
        let store = SavedCurves {
            dir: d.path().join("Curves"),
        };
        e.curves.saved = Some(Vec::new());
        e.document.recipe.curve = BuiltinCurve::StrongContrast.curve();
        // Points on Lightroom's 0–255 steps, as saved curves keep them.
        e.document.recipe.effects.channels[1].points =
            vec![[0., 0.], [128. / 255., 153. / 255.], [1., 1.]];
        let saved_recipe = e.document.recipe.clone();
        e.save_point_curve(&store, "Green Lift");
        assert_eq!(e.status, "Point curve Green Lift saved");
        let saved = e.saved_curves();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].name, "Green Lift");
        // Saving again under the same name keeps the first.
        e.save_point_curve(&store, "Green Lift");
        assert!(e.status.contains("already saved"), "{}", e.status);

        e.document.recipe = crate::develop::Recipe::default();
        let frame = e.begin_edit_frame();
        e.choose_point_curve(CurveChoice::Saved(saved[0].clone()));
        e.finish_edit_frame(frame, &ctx);
        let r = &e.document.recipe;
        assert_eq!(r.curve, saved_recipe.curve);
        assert_eq!(r.effects.channels, saved_recipe.effects.channels);
        let (steps, applied) = e.document.history.steps();
        assert_eq!(applied, 1);
        assert_eq!(steps[0].value, "Green Lift");
        assert_eq!(crate::presets::curves::shown_name(r, &saved), "Green Lift");
    }
}
