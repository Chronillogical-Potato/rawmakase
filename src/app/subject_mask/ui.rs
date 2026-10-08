//! The Masking drawer's side of Subject and Background: the two actions, what
//! they are waiting for, and what went wrong. Everything stays in the drawer; a
//! selection that fails or is cancelled leaves the photo as it was.
use super::models::download_megabytes;
use super::{Failure, Feature, Prompt, Request, Target};
use crate::app::Editor;
use crate::app::retouch_tool::{control_label, hint, indented};
use crate::app::task::spawn;
use crate::app::worker::Event;
use eframe::egui;

impl Editor {
    /// The Select Subject and Select Background actions, with whatever the running or
    /// failed selection has to say. While the first use is being set up, the actions
    /// give way to a card that says what is needed, in order, and does it.
    pub(in crate::app) fn selection_actions(&mut self, ui: &mut egui::Ui) {
        if let Some(request) = self.selection.running() {
            self.selection_progress(ui, request);
            return;
        }
        if self.selection.prompt.is_some() || self.selection.upgrading.is_some() {
            self.setup_card(ui);
            return;
        }
        let reason = self.selection_unavailable();
        control_label(ui, "Select", |ui| {
            let w = (ui.available_width() - 4.) / 2.;
            for (feature, label) in [
                (Feature::Subject, "Select Subject"),
                (Feature::Background, "Select Background"),
            ] {
                let button = egui::Button::new(label).min_size(egui::vec2(w, 24.));
                let response = ui.add_enabled(reason.is_none(), button);
                let response = match reason {
                    Some(why) => response.on_disabled_hover_text(why),
                    None => response.on_hover_text(match feature {
                        Feature::Subject => "Mask the photo's main subject",
                        Feature::Background => "Mask everything but the photo's main subject",
                    }),
                };
                if response.clicked() {
                    self.request_selection(Request {
                        feature,
                        target: Target::NewMask,
                    });
                }
            }
        });
        if let Some(why) = reason {
            hint(ui, why);
        }
        if let Some((request, failure)) = self.selection.failure.clone() {
            self.selection_failure(ui, request, &failure);
        }
        self.model_footer(ui);
    }
    /// What the first use needs, as a highlighted card in the place the actions were:
    /// the steps with the finished ones ticked, and the button for the next.
    fn setup_card(&mut self, ui: &mut egui::Ui) {
        let request = match self.selection.prompt {
            Some(Prompt::Model(r) | Prompt::Upgrade(r)) => Some(r),
            None => self.selection.upgrading.map(|(_, r)| r),
        };
        let Some(request) = request else { return };
        let upgraded = self
            .library
            .as_ref()
            .and_then(|l| l.session.catalog.supports_raster_masks().ok())
            .unwrap_or(false);
        let model = self.selection.models.installed();
        let upgrading = self.selection.upgrading.is_some();
        let installing = self.selection.models.busy();
        let accent = egui::Color32::from_rgb(96, 150, 230);
        egui::Frame::new()
            .fill(egui::Color32::from_rgb(34, 44, 62))
            .stroke(egui::Stroke::new(1.5, accent))
            .corner_radius(5.)
            .inner_margin(egui::Margin::same(9))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    egui::RichText::new(format!(
                        "Before “Select {}” can run",
                        request.feature.name()
                    ))
                    .strong()
                    .size(12.5),
                );
                ui.add_space(4.);
                let step = |ui: &mut egui::Ui, done: bool, current: bool, text: &str| {
                    ui.horizontal(|ui| {
                        let (mark, color) = if done {
                            ("✓", egui::Color32::from_rgb(110, 200, 130))
                        } else if current {
                            ("▶", accent)
                        } else {
                            ("○", egui::Color32::GRAY)
                        };
                        ui.label(egui::RichText::new(mark).color(color).strong());
                        ui.label(egui::RichText::new(text).color(if done {
                            egui::Color32::GRAY
                        } else {
                            egui::Color32::WHITE
                        }));
                    });
                };
                step(ui, upgraded, !upgraded, "Upgrade this catalog");
                step(
                    ui,
                    model,
                    upgraded && !model,
                    &format!("Download the selection model ({} MB)", download_megabytes()),
                );
                ui.add_space(6.);
                if !upgraded {
                    self.upgrade_step(ui, request, upgrading);
                } else if !model {
                    self.model_step(ui, installing);
                }
                if !super::worker::runtime_present() {
                    ui.add_space(4.);
                    ui.colored_label(
                        egui::Color32::from_rgb(230, 170, 90),
                        "This copy of RAWmakase has no ONNX Runtime library, so selecting \
                         cannot run yet.",
                    );
                }
            });
    }
    fn upgrade_step(&mut self, ui: &mut egui::Ui, request: Request, upgrading: bool) {
        let small = |text: &str| {
            egui::RichText::new(text)
                .size(11.)
                .color(egui::Color32::LIGHT_GRAY)
        };
        ui.add(
            egui::Label::new(small(
                "Masks made from a selection are stored in the catalog, which needs a newer \
                 format. RAWmakase first saves a backup copy beside the catalog (as large as \
                 it), then upgrades it. Older versions of RAWmakase cannot open an upgraded \
                 catalog, and it must not be open on another computer or in another copy of \
                 RAWmakase.",
            ))
            .wrap(),
        );
        ui.add_space(6.);
        if upgrading {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Backing up and upgrading the catalog…");
            });
            return;
        }
        ui.horizontal(|ui| {
            let go = egui::Button::new(egui::RichText::new("Upgrade catalog…").strong())
                .fill(egui::Color32::from_rgb(52, 98, 170));
            if ui.add(go).clicked() {
                self.upgrade_catalog(request);
            }
            if ui.button("Not now").clicked() {
                self.selection.prompt = None;
            }
        });
    }
    fn model_step(&mut self, ui: &mut egui::Ui, installing: bool) {
        if let Some((done, total)) = self.selection.models.progress() {
            ui.add(
                egui::ProgressBar::new(done as f32 / total.max(1) as f32)
                    .desired_width(ui.available_width())
                    .text(format!("{} of {} MB", done / 1_000_000, total / 1_000_000)),
            );
            if ui.button("Cancel download").clicked() {
                self.selection.models.cancel();
            }
            return;
        }
        if installing {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Working…");
            });
            return;
        }
        ui.add(
            egui::Label::new(
                egui::RichText::new(
                    "The model runs on this computer; no photo leaves it. It is a one-time \
                     download and needs about that much free disk space.",
                )
                .size(11.)
                .color(egui::Color32::LIGHT_GRAY),
            )
            .wrap(),
        );
        ui.add_space(6.);
        ui.horizontal(|ui| {
            let go = egui::Button::new(egui::RichText::new("Download").strong())
                .fill(egui::Color32::from_rgb(52, 98, 170));
            if ui.add(go).clicked() {
                self.install_model(None);
            }
            if ui
                .button("Import model…")
                .on_hover_text("Use the model file from elsewhere (a copy you transferred)")
                .clicked()
            {
                let (tx, ctx) = (self.tx.clone(), self.context.clone());
                spawn(
                    tx,
                    ctx,
                    |tx| {
                        if let Some(path) = rfd::FileDialog::new()
                            .set_title("Import the selection model")
                            .add_filter("ONNX model", &["onnx"])
                            .pick_file()
                        {
                            let _ = tx.send(Event::ModelFile(path));
                        }
                    },
                    |_, _| {},
                );
            }
            if ui.button("Not now").clicked() {
                self.selection.prompt = None;
            }
        });
    }
    /// Remove, once the model is installed.
    fn model_footer(&mut self, ui: &mut egui::Ui) {
        if self.selection.models.installed() && self.selection.running().is_none() {
            indented(ui, |ui| {
                let remove = ui
                    .small_button("Remove selection model")
                    .on_hover_text(
                        "Delete the downloaded model. Saved masks keep working; selecting \
                         again asks to download it.",
                    )
                    .clicked();
                if remove {
                    let unload = self.selection.worker.unloader();
                    self.selection
                        .models
                        .remove(unload, self.tx.clone(), self.context.clone());
                }
            });
        }
    }
    fn selection_progress(&mut self, ui: &mut egui::Ui, request: Request) {
        control_label(ui, "Select", |ui| {
            ui.spinner();
            // No percentage: the runtime reports none for a run.
            ui.label(format!(
                "Selecting {}…",
                request.feature.name().to_lowercase()
            ));
            if ui
                .button("Cancel")
                .on_hover_text("Stop selecting; the photo is left as it was")
                .clicked()
            {
                self.cancel_selection();
            }
        });
    }
    fn selection_failure(&mut self, ui: &mut egui::Ui, request: Request, failure: &Failure) {
        if *failure == Failure::ModelMissing {
            self.selection.failure = None;
            self.selection.prompt = Some(Prompt::Model(request));
            return;
        }
        hint(ui, &failure.message());
        indented(ui, |ui| {
            if ui.button("Try again").clicked() {
                self.selection.failure = None;
                self.request_selection(request);
            }
            if ui.button("Dismiss").clicked() {
                self.selection.failure = None;
            }
        });
    }
    /// Subject and Background entries for a mask's Add, Subtract and Intersect menus.
    pub(in crate::app) fn selection_menu(
        &mut self,
        ui: &mut egui::Ui,
        mask: usize,
        op: crate::model::masks::MaskOp,
    ) {
        ui.separator();
        let reason = self.selection_unavailable();
        for (feature, label) in [
            (Feature::Subject, "Subject"),
            (Feature::Background, "Background"),
        ] {
            let response = ui.add_enabled(
                reason.is_none() && self.selection.running().is_none(),
                egui::Button::new(label),
            );
            let response = match reason {
                Some(why) => response.on_disabled_hover_text(why),
                None => response,
            };
            if response.clicked() {
                self.request_selection(Request {
                    feature,
                    target: Target::Component { mask, op },
                });
                ui.close();
            }
        }
    }
    /// The selected generated component's source and Regenerate.
    pub(in crate::app) fn regenerate_ui(
        &mut self,
        ui: &mut egui::Ui,
        mask: usize,
        component: usize,
        invert: bool,
        source: &str,
    ) {
        hint(ui, source);
        let reason = self.selection_unavailable();
        let busy = self.selection.running().is_some();
        indented(ui, |ui| {
            let response =
                ui.add_enabled(reason.is_none() && !busy, egui::Button::new("Regenerate"));
            let response = match reason {
                Some(why) => response.on_disabled_hover_text(why),
                None => response.on_hover_text(
                    "Select again and replace only this part of the mask; its other parts and \
                     the adjustments stay",
                ),
            };
            if response.clicked() {
                self.request_selection(Request {
                    feature: if invert {
                        Feature::Background
                    } else {
                        Feature::Subject
                    },
                    target: Target::Regenerate { mask, component },
                });
            }
        });
        if self.selection.running().is_some() {
            self.selection_progress(ui, self.selection.running().expect("checked"));
        }
        if let Some((request, failure)) = self.selection.failure.clone() {
            self.selection_failure(ui, request, &failure);
        }
        if self.selection.prompt.is_some() || self.selection.upgrading.is_some() {
            self.setup_card(ui);
        }
    }
}
