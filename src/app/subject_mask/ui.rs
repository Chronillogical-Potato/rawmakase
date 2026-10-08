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
    /// failed selection has to say.
    pub(in crate::app) fn selection_actions(&mut self, ui: &mut egui::Ui) {
        if let Some(request) = self.selection.running() {
            self.selection_progress(ui, request);
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
        self.selection_prompt(ui);
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
    fn selection_prompt(&mut self, ui: &mut egui::Ui) {
        if let Some((request, failure)) = self.selection.failure.clone() {
            self.selection_failure(ui, request, &failure);
        }
        match self.selection.prompt {
            Some(Prompt::Model(request)) => self.model_prompt(ui, request),
            Some(Prompt::Upgrade(request)) => self.upgrade_prompt(ui, request),
            None => {}
        }
        if self.selection.upgrading.is_some() {
            hint(ui, "Backing up and upgrading the catalog…");
        }
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
    fn model_prompt(&mut self, ui: &mut egui::Ui, request: Request) {
        if let Some((done, total)) = self.selection.models.progress() {
            indented(ui, |ui| {
                ui.add(
                    egui::ProgressBar::new(done as f32 / total.max(1) as f32)
                        .desired_width(ui.available_width() - 70.)
                        .text(format!("{} of {} MB", done / 1_000_000, total / 1_000_000)),
                );
                if ui.button("Cancel").clicked() {
                    self.selection.models.cancel();
                }
            });
            return;
        }
        let mb = download_megabytes();
        hint(
            ui,
            &format!(
                "Selecting runs a model on this computer. It is a one-time download of \
                 {mb} MB and needs about that much free disk space. No photo leaves \
                 this computer."
            ),
        );
        if !super::worker::runtime_present() {
            hint(
                ui,
                "This copy of RAWmakase does not include the ONNX Runtime library the model \
                 needs, so selecting will not run yet.",
            );
        }
        indented(ui, |ui| {
            if ui.button("Download").clicked() {
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
        let _ = request;
    }
    fn upgrade_prompt(&mut self, ui: &mut egui::Ui, request: Request) {
        if self.selection.upgrading.is_some() {
            return;
        }
        hint(
            ui,
            "Masks made from a selection need an upgraded catalog. RAWmakase first saves a \
             backup copy of this catalog beside it (as large as the catalog), then upgrades \
             it. Older versions of RAWmakase cannot open an upgraded catalog, and it must not \
             be open on another computer or in another copy of RAWmakase.",
        );
        indented(ui, |ui| {
            if ui.button("Upgrade catalog…").clicked() {
                self.upgrade_catalog(request);
            }
            if ui.button("Not now").clicked() {
                self.selection.prompt = None;
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
        self.selection_prompt(ui);
    }
}
