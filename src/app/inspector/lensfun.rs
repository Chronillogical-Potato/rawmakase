//! Lens Corrections › Profile's Lensfun parts: the card that offers Lensfun's
//! community profiles where no imported profile fits (a download first, then the
//! lens this photo matches), the Lensfun lenses in the Make, Model and Profile menus,
//! and resolving or updating the lens an edit names. Choosing one is an edit like
//! any other, one History step on the photo shown; the download only installs.
use super::control_row;
use crate::app::design::{Tone, space, text};
use crate::app::lensfun::Job;
use crate::app::theme;
use crate::camera_data::Metadata;
use crate::lens::lensfun::choice::{LICENSE, LICENSE_URI, camera_for, frame_size, photo_of};
use crate::lens::lensfun::installed::Installed;
use crate::lens::lensfun::matching::{self, Outcome};
use crate::lens::lensfun::{Lens, LensfunChoice};
use crate::model::recipe::{ProfileCorrections, Recipe};
use crate::optics::lensfun::Components;
use eframe::egui;
use std::sync::Arc;

/// What the panel knows of Lensfun's database this frame.
pub(in crate::app) struct View {
    pub db: Option<Arc<Installed>>,
    pub running: Option<Job>,
    pub progress: Option<(u64, u64)>,
    pub failure: Option<String>,
    pub suggest: bool,
}

/// What the user asked for that the panel can't do itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum Action {
    Download,
    Cancel,
    /// Stop suggesting Lensfun profiles (Preferences turns it on again).
    Dismiss,
}

/// The lenses a photo may choose from: those fitting its camera and frame.
pub(super) fn choices<'d>(db: &'d Installed, m: &Metadata) -> Vec<&'d Lens> {
    let photo = photo_of(m, frame_size(m));
    matching::choices(&db.db, &photo)
        .1
        .into_iter()
        .map(|i| &db.db.lenses[i])
        .filter(|l| l.refusal().is_none())
        .collect()
}

/// The menu label of a Lensfun lens: "Lensfun", with its calibration when the lens
/// has several.
pub(super) fn profile_label(lens: &Lens, all: &[&Lens]) -> String {
    let same = all
        .iter()
        .filter(|l| l.maker.id == lens.maker.id && l.model.id == lens.model.id)
        .count();
    if same > 1 {
        format!(
            "Lensfun (crop {:.2}, {})",
            lens.calibration.crop,
            aspect_text(lens.calibration.aspect)
        )
    } else {
        "Lensfun".into()
    }
}
fn aspect_text(aspect: f32) -> String {
    for (name, ratio) in [
        ("3:2", 1.5f32),
        ("4:3", 4. / 3.),
        ("16:9", 16. / 9.),
        ("1:1", 1.),
    ] {
        if (aspect - ratio).abs() < 0.01 {
            return name.into();
        }
    }
    format!("{aspect:.2}:1")
}

/// Chooses `lens` for photo `m`: Custom, with its calibration copied, and Enable
/// Profile Corrections on.
pub(super) fn choose(r: &mut Recipe, m: &Metadata, db: &Installed, lens: &Lens) {
    r.set_profile_corrections(m, ProfileCorrections::On);
    r.lens_profile_choice
        .choose_lensfun(LensfunChoice::of(lens, &db.db, camera_for(&db.db, m)));
}

/// The card under the Profile menus. Returns what it can't do itself.
pub(super) fn card(ui: &mut egui::Ui, r: &mut Recipe, m: &Metadata, view: &View) -> Option<Action> {
    if m.is_rendered() {
        return None;
    }
    if let Some(chosen) = r.lens_profile_choice.lensfun().cloned() {
        return chosen_card(ui, r, m, view, &chosen);
    }
    // Only where no imported Adobe profile fits this lens, and while suggestions
    // are on.
    if !view.suggest || m.lens_profiles.auto(m).is_some() {
        return None;
    }
    let builtin = [m.lens.as_ref(), m.lens_stand_in.as_ref()]
        .into_iter()
        .flatten()
        .map(Components::of)
        .fold(Components::default(), |a, b| Components {
            distortion: a.distortion || b.distortion,
            chromatic: a.chromatic || b.chromatic,
            vignetting: a.vignetting || b.vignetting,
        });
    if builtin.distortion && builtin.chromatic && builtin.vignetting {
        return None;
    }
    let Some(db) = &view.db else {
        return download_card(ui, view);
    };
    let photo = photo_of(m, frame_size(m));
    let matched = matching::find(&db.db, &photo);
    let mut action = None;
    frame(ui, |ui| {
        match &matched.outcome {
            Outcome::Unique(i) => {
                let lens = &db.db.lenses[*i];
                if let Some(why) = lens.refusal() {
                    note(
                        ui,
                        &format!(
                            "Lensfun's profile for this lens, {}, is {why}.",
                            lens.label()
                        ),
                    );
                    return;
                }
                let gives = given(lens, db, m);
                let adds = Components {
                    distortion: gives.distortion && !builtin.distortion,
                    chromatic: gives.chromatic && !builtin.chromatic,
                    vignetting: gives.vignetting && !builtin.vignetting,
                };
                if !adds.any() {
                    return;
                }
                strong(ui, &format!("Lensfun: {}", lens.label()));
                note(
                    ui,
                    &format!("Corrects {} for this photo.", gives.describe()),
                );
                ui.horizontal(|ui| {
                    if ui.button("Use").clicked() {
                        choose(r, m, db, lens);
                    }
                    if ui.small_button("Don't show").clicked() {
                        action = Some(Action::Dismiss);
                    }
                });
            }
            Outcome::Ambiguous(all) => {
                strong(ui, &format!("Lensfun: {} possible lenses", all.len()));
                note(
                    ui,
                    "The lens's name fits more than one Lensfun profile; choose the one used.",
                );
                let lenses: Vec<&Lens> = all.iter().map(|i| &db.db.lenses[*i]).collect();
                pick(ui, r, m, db, "lensfun-ambiguous", "Choose…", &lenses);
            }
            Outcome::UnknownCamera if matched.crop.is_none() => {
                note(
                    ui,
                    "Lensfun doesn't know this camera, and the photo records no 35 mm equivalent focal length to tell its crop factor, so no Lensfun profile can be applied.",
                );
            }
            Outcome::NoMatch | Outcome::NoLensName | Outcome::UnknownCamera => {
                note(
                    ui,
                    match matched.outcome {
                        Outcome::UnknownCamera => {
                            "Lensfun doesn't know this camera; you can choose a lens yourself."
                        }
                        Outcome::NoLensName => {
                            "This photo doesn't name its lens; you can choose it yourself."
                        }
                        _ => "No Lensfun profile matched this lens; you can choose one yourself.",
                    },
                );
                let lenses = choices(db, m);
                pick(ui, r, m, db, "lensfun-manual", "Choose manually…", &lenses);
            }
        }
        if action.is_none()
            && !matches!(matched.outcome, Outcome::Unique(_))
            && ui.small_button("Don't show").clicked()
        {
            action = Some(Action::Dismiss);
        }
    });
    action
}

/// The corrections `lens` gives photo `m`.
fn given(lens: &Lens, db: &Installed, m: &Metadata) -> Components {
    LensfunChoice::of(lens, &db.db, camera_for(&db.db, m))
        .apply(m)
        .correction
        .as_ref()
        .map(Components::of)
        .unwrap_or_default()
}

/// Before Lensfun is installed: what it is, the download and its terms.
fn download_card(ui: &mut egui::Ui, view: &View) -> Option<Action> {
    let mut action = None;
    frame(ui, |ui| {
        strong(ui, "Lensfun community profiles may cover this lens.");
        if let Some((done, total)) = view.progress.filter(|_| view.running.is_some()) {
            let fraction = if total > 0 {
                done as f32 / total as f32
            } else {
                0.
            };
            ui.add(
                egui::ProgressBar::new(fraction)
                    .desired_width(ui.available_width())
                    .text(format!("{:.1} MB", done as f32 / 1e6)),
            );
            if ui.button("Cancel").clicked() {
                action = Some(Action::Cancel);
            }
            return;
        }
        if let Some(why) = &view.failure {
            note(ui, &format!("The download failed: {why}"));
        }
        ui.horizontal(|ui| {
            let label = if view.failure.is_some() {
                "Retry".to_string()
            } else {
                format!(
                    "Download Lensfun profiles ({:.1} MB)",
                    crate::lensfun_install::ABOUT_MEGABYTES
                )
            };
            if ui
                .add_enabled(view.running.is_none(), egui::Button::new(label))
                .clicked()
            {
                action = Some(Action::Download);
            }
            if ui.small_button("Don't show").clicked() {
                action = Some(Action::Dismiss);
            }
        });
        terms(ui);
    });
    action
}

/// The lens the edit chose: unresolved ones offer the download or resolving it,
/// resolved ones a newer calibration when the database has one.
fn chosen_card(
    ui: &mut egui::Ui,
    r: &mut Recipe,
    m: &Metadata,
    view: &View,
    chosen: &LensfunChoice,
) -> Option<Action> {
    let found = view
        .db
        .as_ref()
        .and_then(|db| db.db.find(&chosen.lens).map(|lens| (db, lens)));
    if !chosen.is_resolved() {
        let mut action = None;
        frame(ui, |ui| match (&view.db, found) {
            (None, _) => {
                note(
                    ui,
                    &format!(
                        "This edit names the Lensfun profile \"{}\", which needs Lensfun's profiles to be installed.",
                        chosen.name()
                    ),
                );
                if let Some((done, total)) = view.progress.filter(|_| view.running.is_some()) {
                    let fraction = if total > 0 {
                        done as f32 / total as f32
                    } else {
                        0.
                    };
                    ui.add(egui::ProgressBar::new(fraction).desired_width(ui.available_width()));
                    if ui.button("Cancel").clicked() {
                        action = Some(Action::Cancel);
                    }
                } else if ui
                    .add_enabled(
                        view.running.is_none(),
                        egui::Button::new("Download Lensfun Profiles"),
                    )
                    .clicked()
                {
                    action = Some(Action::Download);
                }
                terms(ui);
            }
            (Some(db), Some(_)) => {
                note(
                    ui,
                    &format!(
                        "Resolve \"{}\" against the installed Lensfun profiles to apply it.",
                        chosen.name()
                    ),
                );
                if ui.button("Resolve").clicked()
                    && let Some(resolved) = chosen.resolved(&db.db, m)
                {
                    r.lens_profile_choice.choose_lensfun(resolved);
                }
            }
            (Some(_), None) => note(
                ui,
                &format!(
                    "The installed Lensfun profiles don't have \"{}\"; it may be in a newer version (Preferences › Profiles & Presets).",
                    chosen.name()
                ),
            ),
        });
        return action;
    }
    // A newer calibration of the same lens.
    if let Some((db, lens)) = found
        && chosen.calibration.as_deref() != Some(&lens.calibration)
        && db.db.timestamp > chosen.provenance.database
    {
        frame(ui, |ui| {
            note(
                ui,
                &format!(
                    "A newer Lensfun calibration of this lens is installed ({}).",
                    db.manifest.date()
                ),
            );
            if ui.button("Update").clicked()
                && let Some(resolved) = chosen.resolved(&db.db, m)
            {
                r.lens_profile_choice.choose_lensfun(resolved);
            }
        });
    }
    None
}

/// A menu of `lenses` that chooses the one picked.
fn pick(
    ui: &mut egui::Ui,
    r: &mut Recipe,
    m: &Metadata,
    db: &Installed,
    id: &str,
    label: &str,
    lenses: &[&Lens],
) {
    if lenses.is_empty() {
        note(
            ui,
            "Lensfun has no profile for this camera's mount and sensor size.",
        );
        return;
    }
    let mut chosen = None;
    egui::ComboBox::from_id_salt(id)
        .width(ui.available_width())
        .selected_text(label)
        .height(320.)
        .show_ui(ui, |ui| {
            for (i, lens) in lenses.iter().enumerate() {
                let text = format!("{} · {}", lens.label(), profile_label(lens, lenses));
                if ui.selectable_label(false, text).clicked() {
                    chosen = Some(i);
                }
            }
        });
    if let Some(i) = chosen {
        choose(r, m, db, lenses[i]);
    }
}

/// Lensfun's terms, as every download offer shows them.
pub(in crate::app) fn terms(ui: &mut egui::Ui) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 0.;
        small(ui, "Lensfun database by Lensfun contributors, ");
        ui.hyperlink_to(egui::RichText::new(LICENSE).size(text::SMALL), LICENSE_URI);
        small(ui, " · from ");
        ui.hyperlink_to(
            egui::RichText::new("lensfun.github.io").size(text::SMALL),
            "https://lensfun.github.io",
        );
    });
}

fn frame(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    control_row(ui, "", |ui| {
        let palette = theme::palette(ui.ctx());
        egui::Frame::new()
            .fill(palette.tone(Tone::CARD))
            .stroke(egui::Stroke::new(1., palette.tone(Tone::RAISED)))
            .corner_radius(6.)
            .inner_margin(egui::Margin::same(space::S as i8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = space::XS;
                add(ui);
            });
    });
}
fn strong(ui: &mut egui::Ui, text: &str) {
    ui.add(egui::Label::new(egui::RichText::new(text).size(text::SMALL).strong()).wrap());
}
fn note(ui: &mut egui::Ui, text: &str) {
    ui.add(
        egui::Label::new(
            egui::RichText::new(text)
                .size(text::SMALL)
                .color(theme::palette(ui.ctx()).tone(Tone::SECONDARY)),
        )
        .wrap(),
    );
}
fn small(ui: &mut egui::Ui, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .size(text::SMALL)
            .color(theme::palette(ui.ctx()).tone(Tone::SOFT)),
    );
}
