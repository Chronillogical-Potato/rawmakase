//! Asking to share anonymous usage stats, once, and the Preferences row that
//! changes the answer. `crate::stats` builds and sends the report.
use super::Editor;
use super::widgets::{modal_frame, primary_button};
use crate::app::theme;
use crate::stats::Report;
use eframe::egui::{self, Color32, Vec2};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

const WIDTH: f32 = 320.;

#[derive(Default)]
pub(super) struct UsageStats {
    /// The user's answer; None until they give one, which leaves sharing off.
    pub(super) consent: Option<bool>,
    /// What the reporting thread reads before each report.
    enabled: Arc<AtomicBool>,
    /// Collected in the background; None inside on a platform the service
    /// doesn't count.
    report: crate::stats::Lazy,
    /// The window's graphics backend, for the report.
    gpu: &'static str,
    /// The environment variable that turned sharing off, if any.
    blocked: Option<&'static str>,
    /// Where the answer is saved and reports are recorded: the app data
    /// folder in a real session, None in isolated tests, which never report.
    dir: Option<std::path::PathBuf>,
    /// The last answer couldn't be saved, so it wasn't changed.
    save_failed: bool,
}

impl UsageStats {
    /// `live` (a real session's data folder and window) reads the saved
    /// answer and starts reporting; without it nothing is read or sent.
    pub(super) fn new(
        adapter: Option<&wgpu::AdapterInfo>,
        live: Option<(std::path::PathBuf, &egui::Context)>,
    ) -> Self {
        let (dir, ctx) = live.unzip();
        let stats = Self {
            consent: dir.as_deref().and_then(crate::stats::saved_consent),
            blocked: crate::stats::blocked_by_environment(),
            gpu: crate::stats::gpu_name(adapter),
            dir,
            ..Default::default()
        };
        if let Some(ctx) = ctx {
            let ctx = ctx.clone();
            crate::stats::collect_soon(stats.report.clone(), stats.gpu, move || {
                ctx.request_repaint()
            });
        } else {
            stats.report.get_or_init(|| Report::collect(stats.gpu));
        }
        stats.enabled.store(stats.sharing(), Ordering::Relaxed);
        stats.report_soon(crate::stats::AT_LAUNCH);
        stats
    }

    /// The report once collected; None until then and on a platform the
    /// service doesn't count.
    fn report(&self) -> Option<&Report> {
        self.report.get().and_then(Option::as_ref)
    }

    /// This week's report, if sharing is on and it is due.
    fn report_soon(&self, delay: std::time::Duration) {
        if let Some(dir) = &self.dir
            && self.sharing()
        {
            crate::stats::report_soon(
                dir.clone(),
                self.report.clone(),
                self.gpu,
                self.enabled.clone(),
                delay,
            );
        }
    }

    /// Whether reports go out: the user agreed and nothing overrides that.
    fn sharing(&self) -> bool {
        self.consent == Some(true) && self.blocked.is_none()
    }

    /// Saves and applies an answer; if saving fails the answer is left as
    /// it was, so an opt-out can't revert at the next launch. Sharing turned
    /// on reports this week straight away.
    fn set(&mut self, share: bool) {
        if let Some(dir) = &self.dir
            && crate::stats::save_consent(dir, share).is_err()
        {
            self.save_failed = true;
            return;
        }
        self.save_failed = false;
        self.consent = Some(share);
        self.enabled.store(self.sharing(), Ordering::Relaxed);
        self.report_soon(std::time::Duration::ZERO);
    }

    /// The question hasn't been answered and could be.
    fn unasked(&self) -> bool {
        self.consent.is_none() && self.blocked.is_none() && self.report().is_some()
    }
}

impl Editor {
    /// The one-time question, in the update notice's place under the
    /// toolbar. Waits for first-run setup, modal windows and the update
    /// notice. Only Share turns sharing on.
    pub(super) fn usage_stats_notice(&mut self, ctx: &egui::Context, modal: bool) {
        let palette = theme::palette(ctx);
        if !self.stats.unasked()
            || self.session_file.is_none()
            || !self.onboarding_done
            || modal
            || self.onboarding.visible
            || self.update_notice_shown()
        {
            return;
        }
        let mut answer = None;
        let stats = &self.stats;
        egui::Area::new(egui::Id::new("usage-stats-notice"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::RIGHT_TOP, Vec2::new(-16., 112.))
            .show(ctx, |ui| {
                modal_frame(&palette)
                    .inner_margin(egui::Margin::same(16))
                    .shadow(egui::Shadow {
                        offset: [0, 6],
                        blur: 24,
                        spread: 0,
                        color: Color32::from_black_alpha(110),
                    })
                    .show(ui, |ui| {
                        ui.set_width(WIDTH);
                        ui.spacing_mut().item_spacing = Vec2::new(8., 6.);
                        ui.label(
                            egui::RichText::new("Help make RAWmakase better")
                                .size(14.)
                                .color(palette.gray(236)),
                        );
                        note(
                            ui,
                            "RAWmakase is new. Help us make it better by sending an \
                             anonymous usage report once a week: the app version, your \
                             system and graphics. No files, photos, settings or identifiers.",
                        );
                        details(ui, stats);
                        ui.add_space(6.);
                        ui.spacing_mut().button_padding = Vec2::new(12., 5.);
                        ui.horizontal(|ui| {
                            if primary_button(ui, "Share").clicked() {
                                answer = Some(true);
                            }
                            let decline =
                                egui::Button::new("Don’t Share").min_size(Vec2::new(0., 30.));
                            if ui.add(decline).clicked() {
                                answer = Some(false);
                            }
                        });
                        if stats.save_failed {
                            note(ui, "Couldn’t save your answer. Try again.");
                        }
                        note(ui, "You can change this in Preferences > General.");
                    });
            });
        if let Some(share) = answer {
            self.stats.set(share);
        }
    }

    /// Preferences > General: the answer, and what it shares.
    pub(super) fn usage_stats_preference(&mut self, ui: &mut egui::Ui) {
        let stats = &mut self.stats;
        let available = stats.blocked.is_none() && stats.report().is_some();
        let mut share = stats.sharing();
        let changed = ui
            .add_enabled(
                available,
                egui::Checkbox::new(&mut share, "Share anonymous usage stats"),
            )
            .changed();
        if changed {
            stats.set(share);
        }
    }

    /// The row under the checkbox: why it's off, or the report and the
    /// public page.
    pub(super) fn usage_stats_details(&mut self, ui: &mut egui::Ui) {
        let stats = &self.stats;
        if let Some(variable) = stats.blocked {
            note(
                ui,
                &format!("Turned off by the {variable} environment variable."),
            );
            return;
        }
        if stats.report().is_none() {
            note(ui, "Not available on this platform.");
            return;
        }
        if stats.save_failed {
            note(ui, "Couldn’t save your answer, so it wasn’t changed.");
        }
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                note(ui, "One report a week, with no identifiers.");
                if ui.link("Totals").clicked() {
                    let _ = crate::platform::web::open(crate::stats::PAGE);
                }
            });
            details(ui, stats);
        });
    }

    /// The update notice is on screen, so the question waits.
    fn update_notice_shown(&self) -> bool {
        self.updates.notice_pending()
    }
}

/// The exact report, always shown where sharing is decided.
fn details(ui: &mut egui::Ui, stats: &UsageStats) {
    let palette = theme::palette(ui.ctx());
    let Some(report) = stats.report() else { return };
    note(ui, "What’s sent:");
    egui::Frame::new()
        .fill(palette.gray(24))
        .corner_radius(4.)
        .inner_margin(egui::Margin::symmetric(8, 6))
        .show(ui, |ui| {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(report.pretty())
                        .monospace()
                        .size(11.)
                        .color(palette.gray(190)),
                )
                .wrap(),
            );
        });
}

fn note(ui: &mut egui::Ui, text: &str) {
    ui.add(
        egui::Label::new(
            egui::RichText::new(text)
                .size(12.)
                .color(theme::palette(ui.ctx()).gray(160)),
        )
        .wrap(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stats(consent: Option<bool>) -> UsageStats {
        let mut stats = UsageStats::new(None, None);
        stats.consent = consent;
        stats.enabled.store(stats.sharing(), Ordering::Relaxed);
        stats
    }

    #[test]
    fn shares_only_after_the_user_agrees() {
        assert!(!stats(None).enabled.load(Ordering::Relaxed));
        assert!(!stats(Some(false)).enabled.load(Ordering::Relaxed));
        let mut agreed = stats(None);
        agreed.set(true);
        assert_eq!(
            agreed.enabled.load(Ordering::Relaxed),
            agreed.blocked.is_none()
        );
        agreed.set(false);
        assert!(!agreed.enabled.load(Ordering::Relaxed));
    }

    #[test]
    fn keeps_the_answer_when_it_cant_be_saved() {
        let dir = tempfile::tempdir().unwrap();
        // A file where the data folder should be: nothing can be saved in it.
        let blocked = dir.path().join("not-a-folder");
        std::fs::write(&blocked, b"").unwrap();
        let mut stats = stats(Some(true));
        stats.dir = Some(blocked);
        stats.set(false);
        assert_eq!(stats.consent, Some(true));
        assert!(stats.save_failed);
    }

    #[test]
    fn saves_the_answer_apart_from_the_session() {
        let dir = tempfile::tempdir().unwrap();
        let mut stats = stats(None);
        stats.dir = Some(dir.path().to_path_buf());
        stats.set(false);
        assert_eq!(crate::stats::saved_consent(dir.path()), Some(false));
    }

    #[test]
    fn asks_once() {
        let unanswered = stats(None);
        assert_eq!(unanswered.unasked(), unanswered.blocked.is_none());
        assert!(!stats(Some(false)).unasked());
        assert!(!stats(Some(true)).unasked());
    }
}
