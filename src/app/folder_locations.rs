//! Folder locations on this computer (see `catalog::locations`): their list
//! in Preferences › Catalog, and the questions changing a root or adding a
//! folder can raise.
use super::Editor;
use super::dialogs::{CatalogDialog, FolderAction};
use super::preferences::{gap, group, hint};
use super::theme;
use super::widgets::{confirm_modal, form_row, pretty_path};
use super::worker::Event;
use crate::catalog::{
    Ambiguity, Catalog, Conflict, FolderLocation, Override, Overrides, RootLocations,
};
use eframe::egui;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// A question only the user can answer before a folder change is made.
pub enum FolderQuestion {
    /// A root moves while folders below it have their own locations here.
    Overrides {
        catalog: PathBuf,
        root: i64,
        path: PathBuf,
        overrides: Vec<Override>,
    },
    /// Folders found on disk that equally close locations claim; asked one
    /// at a time, then added with every choice.
    Ambiguous {
        catalog: PathBuf,
        folder: PathBuf,
        open: Vec<Ambiguity>,
        chosen: Vec<FolderLocation>,
    },
}
/// What a folder change does once settled.
enum FolderJob {
    Relink {
        root: i64,
        path: PathBuf,
        overrides: Overrides,
    },
    Import {
        folder: PathBuf,
        choices: Vec<FolderLocation>,
    },
}

/// The Folder locations list, read when the Catalog page shows.
#[derive(Default)]
pub(super) struct LocationsView {
    roots: Vec<RootLocations>,
    /// Folder ids by root and logical path, for Change….
    folders: HashMap<(i64, String), i64>,
    /// This computer's name, as typed.
    computer: String,
    /// Whether each path shown is there, checked off the UI thread: a
    /// missing network share can take seconds to answer.
    found: Arc<Mutex<HashMap<PathBuf, bool>>>,
    error: Option<String>,
}
impl LocationsView {
    fn load(catalog: &Catalog, ctx: &egui::Context) -> Self {
        let roots = match catalog.folder_locations() {
            Ok(roots) => roots,
            Err(e) => {
                return Self {
                    error: Some(format!("{e:#}")),
                    ..Default::default()
                };
            }
        };
        let folders = catalog
            .folders()
            .unwrap_or_default()
            .into_iter()
            .map(|f| ((f.root, f.relative), f.id))
            .collect();
        let found: Arc<Mutex<HashMap<PathBuf, bool>>> = Default::default();
        let paths: Vec<PathBuf> = roots
            .iter()
            .flat_map(|r| {
                std::iter::once(r.path.clone()).chain(r.overrides.iter().map(|o| o.path.clone()))
            })
            .collect();
        let shared = found.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            for path in paths {
                let there = path.is_dir();
                shared.lock().unwrap().insert(path, there);
                ctx.request_repaint();
            }
        });
        Self {
            roots,
            folders,
            computer: catalog.computer_name().unwrap_or_default(),
            found,
            error: None,
        }
    }
}

impl Editor {
    /// The Folder locations block of Preferences › Catalog.
    pub(super) fn folder_locations_block(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let Some(library) = &self.library else {
            return;
        };
        let view = self
            .preferences
            .locations
            .get_or_insert_with(|| LocationsView::load(&library.catalog, &ctx));
        group(ui, "Folder locations");
        if let Some(error) = &view.error {
            form_row(ui, "", |ui| hint(ui, error));
            return;
        }
        let mut renamed = false;
        form_row(ui, "This computer", |ui| {
            let field = ui.add(egui::TextEdit::singleline(&mut view.computer).desired_width(220.));
            renamed = field.lost_focus();
        });
        form_row(ui, "", |ui| {
            hint(
                ui,
                "Each computer keeps its own location for the catalog's folders. \
                 Changing one here never moves them on another computer.",
            );
        });
        let found = view.found.lock().unwrap().clone();
        let status = |path: &Path| match found.get(path) {
            Some(true) => ("Found", theme::gray(150)),
            Some(false) => ("Offline", egui::Color32::from_rgb(222, 150, 90)),
            None => ("Checking…", theme::gray(120)),
        };
        enum Click {
            Change(FolderAction),
            Clear(i64, String),
        }
        let mut click = None;
        for root in &view.roots {
            let name = Path::new(&root.original)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| root.original.clone());
            ui.add_space(6.);
            egui::Frame::new()
                .fill(theme::gray(36))
                .corner_radius(6.)
                .inner_margin(egui::Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let note = if root.location.is_some() {
                        "Located on this computer".to_string()
                    } else {
                        "Where it was added; not located on this computer".to_string()
                    };
                    match entry(
                        ui,
                        &name,
                        &root.path,
                        &note,
                        status(&root.path),
                        root.location.is_some(),
                    ) {
                        Some(Button::Change) => {
                            click = Some(Click::Change(FolderAction::RelinkRoot(root.root)))
                        }
                        Some(Button::Clear) => click = Some(Click::Clear(root.root, String::new())),
                        None => {}
                    }
                    for over in &root.overrides {
                        ui.add_space(6.);
                        ui.horizontal(|ui| {
                            ui.add_space(16.);
                            ui.vertical(|ui| {
                                let note = format!("Folder of {name}, located separately");
                                let id = view.folders.get(&(root.root, over.relative.clone()));
                                match entry(
                                    ui,
                                    &over.relative,
                                    &over.path,
                                    &note,
                                    status(&over.path),
                                    true,
                                ) {
                                    Some(Button::Change) => {
                                        if let Some(id) = id {
                                            click =
                                                Some(Click::Change(FolderAction::RelinkFolder(*id)))
                                        }
                                    }
                                    Some(Button::Clear) => {
                                        click = Some(Click::Clear(root.root, over.relative.clone()))
                                    }
                                    None => {}
                                }
                            });
                        });
                    }
                    for (computer, relative, path) in &root.elsewhere {
                        let what = if relative.is_empty() {
                            String::new()
                        } else {
                            format!("{relative} at ")
                        };
                        ui.add_space(4.);
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(format!(
                                    "On {computer}: {what}{}",
                                    pretty_path(path)
                                ))
                                .size(11.)
                                .color(theme::gray(120)),
                            )
                            .truncate(),
                        );
                    }
                });
        }
        if view.roots.is_empty() {
            form_row(ui, "", |ui| hint(ui, "No folders yet."));
        }
        gap(ui);
        if renamed {
            let name = view.computer.clone();
            if let Some(library) = &mut self.library
                && let Err(e) = library.catalog.rename_computer(&name)
            {
                self.status = format!("Computer not renamed: {e:#}");
            }
        }
        match click {
            Some(Click::Change(action)) => self.catalog_dialog(CatalogDialog::Folder(action), &ctx),
            Some(Click::Clear(root, relative)) => self.clear_folder_location(root, &relative),
            None => {}
        }
    }
    fn clear_folder_location(&mut self, root: i64, relative: &str) {
        if self.activity.is_busy() || !self.flush() {
            return;
        }
        let Some(library) = &mut self.library else {
            return;
        };
        let result = library
            .catalog
            .clear_folder_location(root, relative)
            .and_then(|()| library.refresh());
        self.status = match result {
            Ok(()) => "Folder location cleared on this computer".into(),
            Err(e) => format!("Folder location not cleared: {e:#}"),
        };
        self.preferences.locations = None;
    }
    /// Asks the pending folder question, then carries out the change.
    pub(super) fn folder_question_window(&mut self, ctx: &egui::Context) {
        let Some(question) = &mut self.folder_question else {
            return;
        };
        match question {
            FolderQuestion::Overrides {
                catalog,
                root,
                path,
                overrides,
            } => {
                let list: Vec<String> = overrides
                    .iter()
                    .map(|o| format!("{}: {}", o.relative, pretty_path(&o.path)))
                    .collect();
                let detail = format!(
                    "These folders have their own location on this computer:\n\n{}\n\n\
                     Keep them there, or clear them so they are found in the new location.",
                    list.join("\n")
                );
                let Some(choice) = confirm_modal(
                    ctx,
                    "folder-overrides",
                    "Keep the folders located separately?",
                    &detail,
                    false,
                    &[
                        ("Cancel", None),
                        ("Clear Overrides", Some(Overrides::Clear)),
                        ("Keep Overrides", Some(Overrides::Keep)),
                    ],
                    None,
                ) else {
                    return;
                };
                let (catalog, root, path) = (catalog.clone(), *root, path.clone());
                self.folder_question = None;
                if let Some(overrides) = choice {
                    self.folder_job(
                        catalog,
                        FolderJob::Relink {
                            root,
                            path,
                            overrides,
                        },
                        ctx,
                    );
                }
            }
            FolderQuestion::Ambiguous {
                catalog,
                folder,
                open,
                chosen,
            } => {
                let Some(ambiguity) = open.first() else {
                    let (catalog, folder, choices) =
                        (catalog.clone(), folder.clone(), std::mem::take(chosen));
                    self.folder_question = None;
                    self.folder_job(catalog, FolderJob::Import { folder, choices }, ctx);
                    return;
                };
                let shown: Vec<String> = ambiguity
                    .directories
                    .iter()
                    .take(5)
                    .map(|d| pretty_path(d))
                    .collect();
                let detail = format!(
                    "{}{}\n\nmatches more than one folder of the catalog on this computer. \
                     Choose the one these photos belong to.",
                    shown.join("\n"),
                    if ambiguity.directories.len() > 5 {
                        format!("\nand {} more", ambiguity.directories.len() - 5)
                    } else {
                        String::new()
                    }
                );
                let labels: Vec<String> = ambiguity
                    .options
                    .iter()
                    .map(|o| {
                        if o.relative.is_empty() {
                            format!("Root {} ({})", o.root, pretty_path(&o.path))
                        } else {
                            format!("{} in root {}", o.relative, o.root)
                        }
                    })
                    .collect();
                let mut buttons: Vec<(&str, Option<usize>)> = vec![("Cancel", None)];
                buttons.extend(
                    labels
                        .iter()
                        .enumerate()
                        .map(|(i, l)| (l.as_str(), Some(i))),
                );
                let Some(choice) = confirm_modal(
                    ctx,
                    "folder-ambiguous",
                    "Which folder of the catalog is this?",
                    &detail,
                    false,
                    &buttons,
                    None,
                ) else {
                    return;
                };
                match choice {
                    Some(i) => {
                        let option = ambiguity.options[i].clone();
                        open.remove(0);
                        chosen.push(option);
                    }
                    None => self.folder_question = None,
                }
            }
        }
    }
    /// Carries out a settled folder change and opens the catalog again.
    fn folder_job(&mut self, catalog: PathBuf, job: FolderJob, ctx: &egui::Context) {
        if !self.activity.begin_dialog() {
            return;
        }
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = (|| -> anyhow::Result<_> {
                let mut cat = Catalog::open(&catalog)?;
                Ok(match job {
                    FolderJob::Relink {
                        root,
                        path,
                        overrides,
                    } => {
                        cat.relink_root_with(root, &path, overrides)?;
                        (true, Default::default(), Vec::new())
                    }
                    FolderJob::Import { folder, choices } => {
                        let added = cat.import_folder(
                            &folder,
                            &crate::catalog::MetadataDefaults::load(),
                            &choices,
                        )?;
                        anyhow::ensure!(
                            added.ambiguous.is_empty(),
                            "The folders changed meanwhile; add the folder again"
                        );
                        (false, added.report, added.conflicts)
                    }
                })
            })();
            let event = match result {
                Ok((relinked, report, conflicts)) => {
                    reopened(&catalog, relinked, &report, &conflicts, &ctx)
                }
                Err(e) => Event::CatalogReady(Err(format!("{e:#}"))),
            };
            let _ = tx.send(event);
            ctx.request_repaint();
        });
    }
}
/// A button of a Folder locations entry.
enum Button {
    Change,
    Clear,
}
/// One root or folder of Folder locations: its name, whether it is found
/// and its buttons on the first line, then where it is and why.
fn entry(
    ui: &mut egui::Ui,
    name: &str,
    path: &Path,
    note: &str,
    status: (&str, egui::Color32),
    clearable: bool,
) -> Option<Button> {
    let mut clicked = None;
    // Buttons and status first, from the right, so a long name can't push
    // them out of the window; the name takes what is left.
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if clearable
            && ui
                .button("Clear")
                .on_hover_text("Forget this location on this computer")
                .clicked()
        {
            clicked = Some(Button::Clear);
        }
        if ui.button("Change…").clicked() {
            clicked = Some(Button::Change);
        }
        ui.add_space(6.);
        ui.label(egui::RichText::new(status.0).size(11.).color(status.1));
        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(name)
                        .size(13.)
                        .strong()
                        .color(theme::gray(230)),
                )
                .truncate(),
            );
        });
    });
    ui.add(
        egui::Label::new(
            egui::RichText::new(pretty_path(path))
                .size(12.)
                .color(theme::gray(200)),
        )
        .truncate(),
    )
    .on_hover_text(path.display().to_string());
    ui.label(egui::RichText::new(note).size(11.).color(theme::gray(125)));
    clicked
}
/// The catalog at `path` opened again after a folder change, with what the
/// change found.
pub(super) fn reopened(
    path: &Path,
    relinked: bool,
    report: &crate::catalog::SidecarReport,
    conflicts: &[Conflict],
    ctx: &egui::Context,
) -> Event {
    Event::CatalogReady(
        crate::app::library::Library::load(path, ctx.clone())
            .map(|mut l| {
                if relinked {
                    l.wait_for_availability();
                    let available = l.available_count();
                    l.message = format!(
                        "Folder relinked. {available} of {} photos are available.",
                        l.photos.len()
                    );
                    if available == 0 {
                        l.message.push_str(
                            " No files matched this location; check that the selected \
                             folder contains the expected subfolders.",
                        );
                    }
                }
                folder_added(&mut l, report, conflicts);
                Box::new(l)
            })
            .map_err(|e| format!("{e:#}")),
    )
}
/// Reports what adding a folder skipped, if anything, on the Library's
/// status line: sidecars that could not be read and folders found elsewhere.
pub(super) fn folder_added(
    library: &mut crate::app::library::Library,
    report: &crate::catalog::SidecarReport,
    conflicts: &[Conflict],
) {
    let mut summary: Vec<String> = report.summary().into_iter().collect();
    let mut detail = report.details();
    if let Some(first) = conflicts.first() {
        summary.push(if conflicts.len() == 1 {
            first.message()
        } else {
            format!(
                "{} folders not added: they are linked elsewhere on this computer",
                conflicts.len()
            )
        });
        for conflict in conflicts {
            if !detail.is_empty() {
                detail.push('\n');
            }
            detail.push_str(&conflict.message());
        }
    }
    if !summary.is_empty() {
        library.set_message_with_detail(format!("Folder added · {}", summary.join(" · ")), detail);
    }
}
