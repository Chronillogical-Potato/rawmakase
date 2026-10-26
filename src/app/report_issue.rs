//! Report an Issue…: the system report to copy into a bug report, and the
//! GitHub issue to open. `crate::diagnostics` builds the report; this
//! captures what the window knows once, when the dialog opens, and probes
//! the system once per process off the UI thread.
//!
//! The two are kept apart: the probe's answer belongs to the process and is
//! kept when the dialog closes, so reopening it while the probe runs waits
//! for the same answer rather than starting another; the window's facts
//! belong to the dialog and are captured again each time it opens. The text
//! is made from both whenever either changes, and that one string is shown
//! and copied.
use super::Editor;
use super::shortcuts::icon_button;
use super::widgets::modal_frame;
use super::worker::Event;
use crate::app::theme;
use crate::diagnostics::{self, Build, System};
use eframe::egui::{self, Color32};
use std::sync::Arc;

const WIDTH: f32 = 560.;

/// Probes the system; replaced in tests.
type Probe = Arc<dyn Fn() -> System + Send + Sync>;
/// Opens a web page; replaced in tests.
type OpenUrl = Arc<dyn Fn(&str) -> anyhow::Result<()> + Send + Sync>;

pub(super) struct ReportIssue {
    dialog: Option<Dialog>,
    build: Build,
    /// The window's GPU, from launch; None when it draws without one.
    gpu: Option<diagnostics::Gpu>,
    /// The system, once probed in this process.
    system: Option<Arc<System>>,
    /// The probe under way; its generation tells its answer from any other.
    task: super::task::Task,
    probe: Probe,
    open_url: OpenUrl,
}

struct Dialog {
    /// What the window knew when the dialog opened.
    window: diagnostics::Window,
    /// The report, exactly as shown and copied.
    text: String,
    note: Option<Note>,
}

#[derive(Clone, Debug, PartialEq)]
enum Note {
    Copied,
    /// The browser's launcher started; it may still fail to open one.
    Opened,
    /// The browser couldn't be opened; the report was copied anyway.
    NotOpened(String),
}

impl ReportIssue {
    pub(super) fn new(adapter: Option<&wgpu::AdapterInfo>) -> Self {
        Self {
            dialog: None,
            build: Build::current(),
            gpu: adapter.map(diagnostics::Gpu::from_adapter),
            system: None,
            task: Default::default(),
            probe: Arc::new(System::probe),
            open_url: Arc::new(crate::platform::web::open),
        }
    }

    pub(super) fn is_open(&self) -> bool {
        self.dialog.is_some()
    }

    /// The system lines while the probe hasn't answered.
    fn system(&self) -> System {
        match &self.system {
            Some(system) => (**system).clone(),
            None => System::unavailable("still collecting"),
        }
    }

    fn refresh(&mut self) {
        let system = self.system();
        if let Some(dialog) = &mut self.dialog {
            dialog.text = diagnostics::report(&self.build, &system, Some(&dialog.window));
        }
    }

    fn collecting(&self) -> bool {
        self.task.is_running()
    }
}

impl Editor {
    /// Opens the dialog from any of its entry points, even while a
    /// background task keeps the rest of the window busy.
    pub(super) fn open_report_issue(&mut self) {
        if self.close_confirm {
            return;
        }
        self.preferences.open = false;
        let window = self.report_window();
        let report = &mut self.report_issue;
        report.dialog = Some(Dialog {
            window,
            text: String::new(),
            note: None,
        });
        report.refresh();
        if report.system.is_none() && !report.collecting() {
            let (generation, _) = report.task.start();
            let probe = report.probe.clone();
            super::task::spawn(
                self.tx.clone(),
                self.context.clone(),
                move |events| {
                    let system = Box::new(probe());
                    let _ = events.send(Event::SystemProbed { generation, system });
                },
                move |events, _| {
                    let system = Box::new(System::unavailable("probe failed"));
                    let _ = events.send(Event::SystemProbed { generation, system });
                },
            );
        }
    }

    pub(super) fn close_report_issue(&mut self) {
        self.report_issue.dialog = None;
    }

    /// A probe that answers at once, so tests never ask the package manager.
    #[cfg(test)]
    pub(super) fn report_issue_test_probe(&mut self) {
        self.report_issue.probe = Arc::new(|| System::unavailable("test"));
    }

    /// What the window knows now, for the report: nothing that names a
    /// file, folder, catalog or profile.
    fn report_window(&self) -> diagnostics::Window {
        diagnostics::Window {
            gpu: self.report_issue.gpu.clone(),
            scale: self.context.pixels_per_point(),
            monitor: super::monitor::report_mode(
                &self.view.monitor,
                self.view.system_display.display(),
            ),
            catalog: self
                .library
                .as_ref()
                .map(|library| (library.session.photos.len(), library.session.folders.len())),
            background_task: self.background_work(),
        }
    }

    /// Whether anything runs that a report should mention: a foreground
    /// task, a catalog opening, an export, a preview build or a folder scan.
    fn background_work(&self) -> bool {
        self.activity.is_busy()
            || self.catalog_work.is_some()
            || self.exporting()
            || self
                .preview_builds
                .progress()
                .is_some_and(|progress| progress.running())
            || self.folder_sync.is_scanning()
    }

    /// The probe answered, or stopped: either way it no longer runs.
    pub(super) fn system_probed(&mut self, generation: u64, system: System) {
        let report = &mut self.report_issue;
        if !report.task.counts(generation) {
            return;
        }
        report.task.finish(generation);
        report.system = Some(Arc::new(system));
        report.refresh();
    }

    pub(super) fn report_issue_window(&mut self, ctx: &egui::Context) {
        let Some(dialog) = &self.report_issue.dialog else {
            return;
        };
        let palette = theme::palette(ctx);
        let collecting = self.report_issue.collecting();
        let (mut copy, mut open, mut close) = (false, false, false);
        let response = egui::Modal::new(egui::Id::new("report-an-issue"))
            .backdrop_color(Color32::from_black_alpha(140))
            .frame(modal_frame(&palette).inner_margin(egui::Margin::symmetric(24, 20)))
            .show(ctx, |ui| {
                ui.set_width(WIDTH);
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("Report an Issue")
                            .size(17.)
                            .color(palette.gray(235)),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        close = icon_button(ui, super::icons::Icon::Close, "Close · Esc").clicked();
                    });
                });
                ui.add_space(8.);
                ui.label(
                    egui::RichText::new(
                        "Found a problem? Check whether it has been reported already. \
                         Otherwise open a new issue on GitHub and paste this system \
                         information into its \u{201c}System information\u{201d} field. \
                         It names no files, folders, catalogs or profiles.",
                    )
                    .color(palette.gray(190)),
                );
                ui.hyperlink_to("Search existing issues", diagnostics::SEARCH_ISSUES);
                ui.add_space(8.);
                egui::ScrollArea::vertical()
                    .max_height(300.)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        // Read-only: it can be selected, never changed.
                        ui.add(
                            egui::TextEdit::multiline(&mut dialog.text.as_str())
                                .id_salt("report-an-issue-text")
                                .font(egui::TextStyle::Monospace)
                                .desired_width(f32::INFINITY),
                        );
                    });
                if collecting {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(
                            egui::RichText::new("Collecting system details…")
                                .color(palette.gray(150)),
                        );
                    });
                }
                if let Some(note) = &dialog.note {
                    let (text, color) = match note {
                        Note::Copied => ("Copied.".to_string(), palette.gray(170)),
                        // The launcher only starts: whether a browser
                        // then opens isn't known, so the address is given.
                        Note::Opened => (
                            format!(
                                "Copied. Paste it into \u{201c}System information\u{201d} \
                                 in the issue. If no browser opened, go to {}",
                                diagnostics::new_issue_url(self.report_issue.build.version)
                            ),
                            palette.gray(170),
                        ),
                        Note::NotOpened(error) => (
                            format!(
                                "The browser couldn't be opened: {error}. The system \
                                 information is copied; open {} yourself.",
                                diagnostics::new_issue_url(self.report_issue.build.version)
                            ),
                            palette.danger(),
                        ),
                    };
                    ui.add(egui::Label::new(egui::RichText::new(text).color(color)).wrap());
                }
                ui.add_space(10.);
                ui.horizontal(|ui| {
                    copy = ui.button("Copy System Info").clicked();
                    open = ui.button("Open GitHub Issue…").clicked();
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        close |= ui.button("Close").clicked();
                    });
                });
            });
        if copy || open {
            self.copy_report(ctx, open);
        }
        if close || response.should_close() {
            self.close_report_issue();
        }
    }

    /// Copies the report as shown; `open` then opens a new issue, its
    /// version filled in, for the user to paste it into.
    fn copy_report(&mut self, ctx: &egui::Context, open: bool) {
        let report = &mut self.report_issue;
        let Some(dialog) = &mut report.dialog else {
            return;
        };
        ctx.copy_text(dialog.text.clone());
        dialog.note = Some(if !open {
            Note::Copied
        } else {
            match (report.open_url)(&diagnostics::new_issue_url(report.build.version)) {
                Ok(()) => Note::Opened,
                Err(error) => Note::NotOpened(error.to_string()),
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Module;
    use crate::app::tests::editor_with_catalog;
    use crate::diagnostics::Field;
    use std::sync::Mutex;
    use std::sync::mpsc::{Receiver, Sender, channel};

    fn system(os: &str) -> System {
        System {
            os: Field::Value(os.into()),
            ..System::unavailable("test")
        }
    }

    /// A probe that answers when the test sends it a system.
    fn held_probe(e: &mut Editor) -> Sender<System> {
        let (send, answers) = channel::<System>();
        let answers = Mutex::new(answers);
        e.report_issue.probe = Arc::new(move || answers.lock().unwrap().recv().unwrap());
        send
    }

    fn opened_urls(e: &mut Editor, fail: bool) -> Receiver<String> {
        let (send, urls) = channel();
        let send = Mutex::new(send);
        e.report_issue.open_url = Arc::new(move |url| {
            send.lock().unwrap().send(url.to_string()).unwrap();
            if fail {
                anyhow::bail!("no browser")
            }
            Ok(())
        });
        urls
    }

    fn input(events: Vec<egui::Event>) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1400., 900.),
            )),
            events,
            ..Default::default()
        }
    }

    /// One whole frame; what it asked the platform to do.
    fn frame(e: &mut Editor, events: Vec<egui::Event>) -> egui::PlatformOutput {
        let ctx = e.context.clone();
        let mut output = ctx.run_ui(input(events), |ui| e.draw(ui));
        output.textures_delta.clear();
        output.platform_output
    }

    /// Frames until the probe's answer has been taken in.
    fn settle(e: &mut Editor) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while e.report_issue.collecting() && std::time::Instant::now() < deadline {
            frame(e, vec![]);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(!e.report_issue.collecting(), "the probe never finished");
    }

    fn text(e: &Editor) -> String {
        e.report_issue.dialog.as_ref().unwrap().text.clone()
    }

    fn copied(output: &egui::PlatformOutput) -> Vec<String> {
        output
            .commands
            .iter()
            .filter_map(|c| match c {
                egui::OutputCommand::CopyText(text) => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    /// Clicks the button labelled `label`, as a person would.
    fn click(e: &mut Editor, label: &str) -> egui::PlatformOutput {
        // Lay the dialog out, then find the button by its text's position.
        frame(e, vec![]);
        let at = find(e, label).unwrap_or_else(|| panic!("no {label}"));
        frame(e, vec![egui::Event::PointerMoved(at)]);
        frame(
            e,
            vec![egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            }],
        );
        frame(
            e,
            vec![egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            }],
        )
    }

    /// Where `label` was drawn, from the shapes egui laid out.
    fn find(e: &mut Editor, label: &str) -> Option<egui::Pos2> {
        let ctx = e.context.clone();
        let mut output = ctx.run_ui(input(vec![]), |ui| e.draw(ui));
        output.textures_delta.clear();
        output
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) if text.galley.text() == label => {
                    Some(text.visual_bounding_rect().center())
                }
                _ => None,
            })
    }

    /// Where the button screen readers know as `label` was drawn, if it is
    /// enabled.
    fn accessible(e: &mut Editor, label: &str) -> Option<(egui::Pos2, bool)> {
        let ctx = e.context.clone();
        ctx.enable_accesskit();
        let mut output = ctx.run_ui(input(vec![]), |ui| e.draw(ui));
        output.textures_delta.clear();
        let update = output.platform_output.accesskit_update?;
        update.nodes.iter().find_map(|(_, node)| {
            (node.label() == Some(label)).then(|| {
                let b = node.bounds().expect("laid out");
                let center = egui::pos2(((b.x0 + b.x1) / 2.) as f32, ((b.y0 + b.y1) / 2.) as f32);
                (center, !node.is_disabled())
            })
        })
    }

    fn press_at(e: &mut Editor, at: egui::Pos2) {
        frame(e, vec![egui::Event::PointerMoved(at)]);
        for pressed in [true, false] {
            frame(
                e,
                vec![egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Default::default(),
                }],
            );
        }
    }

    #[test]
    fn the_sidebar_button_opens_it_while_busy() -> anyhow::Result<()> {
        let (_dir, mut e, _) = editor_with_catalog(&["one.ARW"])?;
        e.onboarding.visible = false;
        e.report_issue_test_probe();
        assert!(e.activity.begin_folder_change());
        frame(&mut e, vec![]);
        // The shortcuts, beside it, are disabled while the folder changes.
        let shortcuts = format!(
            "Keyboard shortcuts · {}",
            crate::app::shortcuts::keys_text("Cmd+/")
        );
        let (keys, enabled) = accessible(&mut e, &shortcuts).expect("the shortcuts button");
        assert!(!enabled);
        press_at(&mut e, keys);
        assert!(!e.view.shortcuts);
        let (bug, enabled) = accessible(&mut e, "Report an issue").expect("the bug button");
        assert!(enabled);
        press_at(&mut e, bug);
        assert!(e.report_issue.is_open());
        settle(&mut e);
        assert!(
            text(&e).contains("Background task: running\n"),
            "{}",
            text(&e)
        );
        Ok(())
    }

    #[test]
    fn preferences_opens_it_in_its_place() -> anyhow::Result<()> {
        let (_dir, mut e, _) = editor_with_catalog(&[])?;
        e.onboarding.visible = false;
        e.report_issue_test_probe();
        e.open_preferences(crate::app::preferences::Tab::General);
        // Help is at the foot of the General page.
        let page = egui::pos2(750., 500.);
        frame(&mut e, vec![egui::Event::PointerMoved(page)]);
        for _ in 0..3 {
            frame(
                &mut e,
                vec![egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0., -10_000.),
                    phase: egui::TouchPhase::Move,
                    modifiers: egui::Modifiers::NONE,
                }],
            );
        }
        let output = click(&mut e, "Report an Issue…");
        assert!(copied(&output).is_empty());
        assert!(e.report_issue.is_open());
        assert!(!e.preferences.open);
        Ok(())
    }

    #[test]
    fn says_whether_a_task_runs_in_the_background() -> anyhow::Result<()> {
        let (_dir, mut e, _) = editor_with_catalog(&[])?;
        e.onboarding.visible = false;
        e.report_issue_test_probe();
        e.open_report_issue();
        assert!(text(&e).contains("Background task: none\n"));
        e.close_report_issue();
        // A catalog opening, which leaves the window's activity idle.
        e.catalog_work = Some("Opening".into());
        assert!(!e.activity.is_busy());
        e.open_report_issue();
        assert!(text(&e).contains("Background task: running\n"));
        Ok(())
    }

    #[test]
    fn copies_exactly_the_text_it_shows() -> anyhow::Result<()> {
        let (_dir, mut e, _) = editor_with_catalog(&["one.ARW", "two.ARW"])?;
        e.onboarding.visible = false;
        e.report_issue.probe = Arc::new(|| system("Test OS 1.0"));
        e.open_report_issue();
        settle(&mut e);
        let shown = text(&e);
        assert!(shown.contains("OS: Test OS 1.0\n"), "{shown}");
        assert!(shown.contains("Catalog: 2 photos, 1 folder\n"), "{shown}");
        let output = click(&mut e, "Copy System Info");
        assert_eq!(copied(&output), vec![shown]);
        assert_eq!(
            e.report_issue.dialog.as_ref().unwrap().note,
            Some(Note::Copied)
        );
        Ok(())
    }

    #[test]
    fn opens_a_short_issue_url_and_copies_the_report() -> anyhow::Result<()> {
        let (_dir, mut e, _) = editor_with_catalog(&[])?;
        e.onboarding.visible = false;
        e.report_issue.probe = Arc::new(|| system("Test OS"));
        let urls = opened_urls(&mut e, false);
        e.open_report_issue();
        settle(&mut e);
        let output = click(&mut e, "Open GitHub Issue…");
        assert_eq!(copied(&output), vec![text(&e)]);
        assert_eq!(
            urls.try_iter().collect::<Vec<_>>(),
            vec![diagnostics::new_issue_url(env!("CARGO_PKG_VERSION"))]
        );
        assert_eq!(
            e.report_issue.dialog.as_ref().unwrap().note,
            Some(Note::Opened)
        );
        // A launcher can start and still open no browser: the address shows.
        let url = diagnostics::new_issue_url(env!("CARGO_PKG_VERSION"));
        let ctx = e.context.clone();
        let mut output = ctx.run_ui(input(vec![]), |ui| e.draw(ui));
        output.textures_delta.clear();
        assert!(
            output.shapes.iter().any(|clipped| matches!(
                &clipped.shape,
                egui::Shape::Text(text) if text.galley.text().contains(&url)
            )),
            "the issue address is shown"
        );
        Ok(())
    }

    #[test]
    fn a_browser_that_fails_to_open_leaves_the_report_copied() -> anyhow::Result<()> {
        let (_dir, mut e, _) = editor_with_catalog(&[])?;
        e.onboarding.visible = false;
        e.report_issue.probe = Arc::new(|| system("Test OS"));
        let urls = opened_urls(&mut e, true);
        e.open_report_issue();
        settle(&mut e);
        let output = click(&mut e, "Open GitHub Issue…");
        assert_eq!(copied(&output), vec![text(&e)]);
        assert_eq!(urls.try_iter().count(), 1);
        assert_eq!(
            e.report_issue.dialog.as_ref().unwrap().note,
            Some(Note::NotOpened("no browser".into()))
        );
        // Copying still works.
        let output = click(&mut e, "Copy System Info");
        assert_eq!(copied(&output), vec![text(&e)]);
        Ok(())
    }

    #[test]
    fn a_probe_that_panics_leaves_a_partial_report() -> anyhow::Result<()> {
        let (_dir, mut e, _) = editor_with_catalog(&["one.ARW"])?;
        e.onboarding.visible = false;
        e.report_issue.probe = Arc::new(|| panic!("package manager crashed"));
        e.open_report_issue();
        assert!(e.report_issue.collecting());
        settle(&mut e);
        let shown = text(&e);
        assert!(
            shown.contains("OS: unavailable (probe failed)\n"),
            "{shown}"
        );
        assert!(shown.starts_with("RAWmakase: "), "{shown}");
        assert!(shown.contains("Catalog: 1 photo, 1 folder\n"), "{shown}");
        // Still open, and copying works.
        assert!(e.report_issue.is_open());
        let output = click(&mut e, "Copy System Info");
        assert_eq!(copied(&output), vec![shown]);
        Ok(())
    }

    #[test]
    fn reopening_while_probing_waits_for_the_same_probe() -> anyhow::Result<()> {
        let (_dir, mut e, _) = editor_with_catalog(&["one.ARW"])?;
        e.onboarding.visible = false;
        let answer = held_probe(&mut e);
        e.open_report_issue();
        let first = e.report_issue.task.id();
        assert!(text(&e).contains("OS: unavailable (still collecting)\n"));
        // Closed and opened again before the probe answers: no second probe.
        e.report_issue.dialog = None;
        e.open_report_issue();
        assert_eq!(e.report_issue.task.id(), first);
        answer.send(system("Probed OS"))?;
        settle(&mut e);
        assert!(text(&e).contains("OS: Probed OS\n"));
        // An answer from no probe of this one's is ignored.
        e.system_probed(first + 7, system("Stale OS"));
        assert!(text(&e).contains("OS: Probed OS\n"));
        Ok(())
    }

    #[test]
    fn a_late_answer_keeps_the_window_facts_the_dialog_opened_with() -> anyhow::Result<()> {
        let (_dir, mut e, _) = editor_with_catalog(&["one.ARW", "two.ARW"])?;
        e.onboarding.visible = false;
        let answer = held_probe(&mut e);
        e.open_report_issue();
        // The catalog changes behind the open dialog (a catalog opened
        // in the background); the dialog keeps the facts it opened with.
        let (_other, other, _) = editor_with_catalog(&["a.ARW", "b.ARW", "c.ARW"])?;
        e.library = other.library;
        answer.send(system("Probed OS"))?;
        settle(&mut e);
        let shown = text(&e);
        assert!(shown.contains("Catalog: 2 photos, 1 folder\n"), "{shown}");
        assert!(shown.contains("OS: Probed OS\n"), "{shown}");
        // Reopened, it reads the window again and keeps the probed system.
        e.report_issue.dialog = None;
        e.open_report_issue();
        let shown = text(&e);
        assert!(shown.contains("Catalog: 3 photos, 1 folder\n"), "{shown}");
        assert!(shown.contains("OS: Probed OS\n"), "{shown}");
        assert!(!e.report_issue.collecting());
        Ok(())
    }

    #[test]
    fn names_no_path_user_catalog_or_profile() -> anyhow::Result<()> {
        let home = tempfile::tempdir()?;
        let user = "alice-private-user";
        let catalog_name = "Secret Wedding Catalog";
        let dir = home.path().join(user).join("Pictures");
        std::fs::create_dir_all(dir.join("photos"))?;
        std::fs::write(dir.join("photos").join("IMG_SECRET_0001.ARW"), b"fixture")?;
        let path = dir.join(format!("{catalog_name}.rawmakase"));
        crate::catalog::Catalog::create(&path)?.add_folder(&dir.join("photos"))?;
        let ctx = egui::Context::default();
        let mut e = Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
        e.library = Some(Box::new(crate::app::library::Library::load(&path, ctx)?));
        e.module = Module::Library;
        e.onboarding.visible = false;
        let icc = home.path().join(user).join("My Calibrated Display.icc");
        e.view.monitor = crate::app::monitor::MonitorProfile::Custom(icc);
        e.report_issue.probe = Arc::new(System::probe);
        e.open_report_issue();
        settle(&mut e);
        let shown = text(&e);
        assert!(
            shown.contains("Monitor profile: Custom ICC profile\n"),
            "{shown}"
        );
        assert!(shown.contains("Catalog: 1 photo, 1 folder\n"), "{shown}");
        let home_dir = std::env::var("HOME").unwrap_or_default();
        let login = std::env::var("USER").unwrap_or_default();
        for private in [
            home.path().to_string_lossy().as_ref(),
            user,
            catalog_name,
            "IMG_SECRET",
            "My Calibrated Display",
            ".icc",
            ".rawmakase",
            // A path's separator after its root: "Debian GNU/Linux" is fine.
            "/tmp",
            "/home",
            "/Users",
        ]
        .into_iter()
        .chain((!home_dir.is_empty()).then_some(home_dir.as_str()))
        .chain((login.len() > 3).then_some(login.as_str()))
        {
            assert!(!shown.contains(private), "{private:?} in {shown}");
        }
        Ok(())
    }

    #[test]
    fn keys_go_to_the_dialog_not_the_photo_behind_it() -> anyhow::Result<()> {
        let (_dir, mut e, ids) = editor_with_catalog(&["one.ARW"])?;
        e.onboarding.visible = false;
        e.library.as_mut().unwrap().select(Some(ids[0]));
        e.report_issue.probe = Arc::new(|| system("Test OS"));
        e.open_report_issue();
        assert!(e.workspace_modal() && e.command_modal());
        let key = |key, pressed| egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed,
            repeat: false,
            modifiers: Default::default(),
        };
        // Pressed and released, so the next press isn't a repeat.
        let tap = |e: &mut Editor, k| {
            frame(e, vec![key(k, true), key(k, false)]);
        };
        tap(&mut e, egui::Key::Num5);
        tap(&mut e, egui::Key::P);
        let photo = e.library.as_ref().unwrap().photo(ids[0]).unwrap();
        assert_eq!(photo.rating, 0);
        assert_eq!(photo.flag, 0);
        assert!(e.report_issue.is_open());
        // Escape closes it; then the same keys reach the photo again.
        tap(&mut e, egui::Key::Escape);
        assert!(!e.report_issue.is_open());
        tap(&mut e, egui::Key::Num5);
        assert_eq!(e.library.as_ref().unwrap().photo(ids[0]).unwrap().rating, 5);
        Ok(())
    }
}
