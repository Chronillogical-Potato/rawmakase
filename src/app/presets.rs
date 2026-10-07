use super::Editor;
use super::bulk_import::ImportKind;
use super::dialogs::FileDialog;
use super::widgets::{section, segmented};
use super::worker::Event;
use crate::app::theme;
use crate::model::recipe::Recipe;
use anyhow::Context as _;
use eframe::egui::{self, Sense, Stroke, Vec2};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long a preset is hovered before the photo previews it.
const HOVER_PREVIEW: Duration = Duration::from_millis(300);

impl Editor {
    pub(super) fn reload_presets(&mut self, ctx: &egui::Context) {
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        let scan = self.presets.next_scan();
        super::task::spawn(
            tx,
            ctx,
            move |tx| {
                let library = Arc::new(crate::presets::load_library());
                let _ = tx.send(Event::XmpLibrary { scan, library });
            },
            move |tx, error| {
                let _ = tx.send(Event::PresetScanFailed { scan, error });
            },
        );
    }
    /// A scan that failed: the presets read before stay, and commands stop waiting.
    pub(super) fn preset_scan_failed(&mut self, scan: u64, error: String) {
        if self.presets.is_latest(scan) {
            self.presets.scanned = true;
            self.status = format!("Presets not read: {error}");
        }
    }
    /// A finished library scan, unless a later one has started since: scans run in
    /// parallel and may finish in any order.
    pub(super) fn presets_scanned(&mut self, scan: u64, library: Arc<crate::presets::Library>) {
        if self.presets.is_latest(scan) {
            self.presets.library = library;
            self.presets.scanned = true;
            // A preset named as a raw default may have been imported or changed.
            if let Err(e) = self.set_raw_defaults(self.raw_defaults.settings().clone()) {
                self.status = format!("Raw defaults not saved: {e:#}");
            }
            self.refresh_preset_support();
        }
    }
    pub(super) fn refresh_preset_support(&mut self) {
        let had_preview = self.presets.preview.take().is_some();
        self.presets.hover = None;
        if had_preview {
            self.schedule();
        }
        // The list depends on which presets fit this photo.
        self.presets.revision += 1;
        let Some(m) = &self.document.metadata else {
            self.presets.issues.clear();
            self.presets.substitutes.clear();
            return;
        };
        let base = Recipe::with_profiles(m, &self.document.profiles);
        self.presets.issues = self
            .presets
            .library
            .presets
            .iter()
            .map(|p| {
                p.apply(&base, m, &self.document.profiles, None)
                    .err()
                    .map(|e| format!("{e:#}"))
            })
            .collect();
        self.presets.substitutes = self
            .presets
            .library
            .presets
            .iter()
            .map(|p| p.profile_substitute(m, &self.document.profiles))
            .collect();
    }
    pub(super) fn presets_ui(&mut self, ui: &mut egui::Ui) {
        let palette = theme::palette(ui.ctx());
        let library = self.presets.library.clone();
        let mut clicked = None;
        let mut hovered = None;
        let mut user_action = None;
        let user = crate::presets::user::UserPresets::default();
        let mut import = None;
        ui.spacing_mut().item_spacing.y = 0.;
        egui::ScrollArea::vertical()
            .id_salt("preset-list")
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                section(ui, "Presets", false, |ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(4., 4.);
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.presets.filter)
                                .hint_text("Search presets")
                                .font(egui::FontId::proportional(12.))
                                .desired_width(ui.available_width() - 26.),
                        );
                        let (rect, response) =
                            ui.allocate_exact_size(Vec2::splat(22.), Sense::click());
                        let color = palette.gray(if response.hovered() { 235 } else { 160 });
                        if response.hovered() {
                            ui.painter().rect_filled(rect, 3., palette.gray(50));
                        }
                        crate::app::icons::paint_at(
                            ui.painter(),
                            crate::app::icons::Icon::Add,
                            rect.center(),
                            14.,
                            color,
                        );
                        let response = response.on_hover_text("Import Lightroom presets (.xmp)");
                        egui::Popup::menu(&response).show(|ui| {
                            ui.set_min_width(190.);
                            if ui
                                .add(egui::Button::new("Import Presets…").frame(false))
                                .on_hover_text("Choose one or more .xmp presets")
                                .clicked()
                            {
                                import = Some(FileDialog::ImportXmp);
                                ui.close();
                            }
                            if ui
                                .add(egui::Button::new("Import Folder…").frame(false))
                                .on_hover_text("Import every preset in a folder and its subfolders")
                                .clicked()
                            {
                                import = Some(FileDialog::ImportFolder(ImportKind::Presets));
                                ui.close();
                            }
                        });
                    });
                    let mut show = match (self.presets.favorites_only, self.presets.compatible_only)
                    {
                        (true, _) => 1,
                        (false, true) => 2,
                        _ => 0,
                    };
                    let w = ui.available_width();
                    if segmented(
                        ui,
                        &mut show,
                        &[(0, "All"), (1, "Favorites"), (2, "Compatible")],
                        w,
                    ) {
                        self.presets.favorites_only = show == 1;
                        self.presets.compatible_only = show == 2;
                    }
                    self.preset_amount_ui(ui);
                    let available = self.presets.issues.iter().filter(|e| e.is_none()).count();
                    // One line whatever the count, so switching photos never
                    // moves the list below.
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(if self.document.metadata.is_some() {
                                format!(
                                    "{available} of {} presets fit this camera",
                                    library.presets.len()
                                )
                            } else {
                                format!("{} presets", library.presets.len())
                            })
                            .size(10.)
                            .color(palette.gray(125)),
                        )
                        .truncate(),
                    );
                    if !library.errors.is_empty() {
                        ui.label(
                            egui::RichText::new(format!(
                                "{} unreadable files",
                                library.errors.len()
                            ))
                            .size(10.)
                            .color(ui.visuals().warn_fg_color),
                        )
                        .on_hover_text(library.errors.join("\n"));
                    }
                    ui.add_space(2.);
                    let groups = self.preset_list();
                    if groups.is_empty() && !library.presets.is_empty() {
                        ui.weak("No presets match these filters.");
                        if ui.small_button("Clear filters").clicked() {
                            self.presets.filter.clear();
                            self.presets.favorites_only = false;
                            self.presets.compatible_only = false;
                        }
                    }
                    ui.spacing_mut().item_spacing.y = 0.;
                    let force_open = !self.presets.filter.is_empty() || self.presets.favorites_only;
                    for PresetGroup {
                        imported,
                        name: group,
                        presets,
                    } in groups.iter()
                    {
                        let id = ui.make_persistent_id(("preset-group", imported, group));
                        let open = force_open
                            || ui.ctx().data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
                        if list_row(ui, group, Some(presets.len()), 0, Some(open), false, true)
                            .clicked()
                            && !force_open
                        {
                            ui.ctx().data_mut(|d| d.insert_temp(id, !open));
                        }
                        if !open {
                            continue;
                        }
                        for (i, name) in presets {
                            let i = *i;
                            let p = &library.presets[i];
                            let issue = self.presets.issues.get(i).and_then(Option::as_ref);
                            // Incompatible presets are grayed out but still apply
                            // everything they can, as in Lightroom.
                            let enabled = self.document.full().is_some();
                            let favorite = self.presets.favorites.contains(&p.id);
                            let response = list_row(
                                ui,
                                name,
                                None,
                                1,
                                None,
                                self.presets.selected == p.id,
                                enabled && issue.is_none(),
                            );
                            // Off screen in the scrolled list: nothing more to draw.
                            if !ui.is_rect_visible(response.rect) {
                                continue;
                            }
                            // Favorite star on the right, shown on hover or when set.
                            let star = egui::Rect::from_center_size(
                                egui::pos2(response.rect.right() - 12., response.rect.center().y),
                                Vec2::splat(16.),
                            );
                            let star_response =
                                ui.interact(star, ui.id().with(("favorite", i)), Sense::click());
                            if favorite || response.hovered() || star_response.hovered() {
                                ui.painter().text(
                                    star.center(),
                                    egui::Align2::CENTER_CENTER,
                                    "★",
                                    egui::FontId::proportional(12.),
                                    palette.gray(if star_response.hovered() {
                                        240
                                    } else if favorite {
                                        200
                                    } else {
                                        110
                                    }),
                                );
                            }
                            if star_response
                                .on_hover_text(if favorite {
                                    "Remove from favorites"
                                } else {
                                    "Add to favorites"
                                })
                                .clicked()
                            {
                                if favorite {
                                    self.presets.favorites.remove(&p.id);
                                } else {
                                    self.presets.favorites.insert(p.id.clone());
                                }
                                self.presets.revision += 1;
                                if let Err(e) =
                                    crate::presets::save_favorites(&self.presets.favorites)
                                {
                                    self.status = e.to_string();
                                }
                                continue;
                            }
                            if enabled && response.clicked() {
                                clicked = Some(i);
                            }
                            // Presets made here can be changed, as Lightroom's own.
                            if user.owns(p) {
                                response.context_menu(|ui| {
                                    use super::user_presets::PresetAction;
                                    if ui.button("Update with Current Settings").clicked() {
                                        user_action = Some(PresetAction::Update(i));
                                    }
                                    if ui.button("Rename…").clicked() {
                                        user_action = Some(PresetAction::StartRename(i));
                                    }
                                    if ui.button("Delete").clicked() {
                                        user_action = Some(PresetAction::Delete(i));
                                    }
                                });
                            }
                            if response.hovered() && enabled {
                                hovered = Some(i);
                            }
                            // Built only while it shows.
                            let substitute =
                                self.presets.substitutes.get(i).and_then(Option::as_ref);
                            response.on_hover_ui(|ui| {
                                ui.label(preset_detail(p, issue, substitute));
                            });
                        }
                    }
                });
                self.snapshots_section(ui);
                self.history_section(ui);
            });
        if let Some(dialog) = import {
            self.dialog(dialog, &ui.ctx().clone());
        }
        if let Some(action) = user_action {
            self.preset_action(action);
        }
        if let Some(i) = clicked {
            // The status bar reports a preset that could not be applied.
            let _ = self.apply_preset(i);
        } else if let Some(i) = hovered {
            if self.presets.hover.as_ref().is_none_or(|(old, _)| *old != i) {
                if self.presets.preview.take().is_some() {
                    self.schedule();
                }
                self.presets.hover = Some((i, Instant::now()));
            }
            if self.presets.preview.is_none()
                && self
                    .presets
                    .hover
                    .as_ref()
                    .is_some_and(|(_, t)| t.elapsed() > HOVER_PREVIEW)
                && let Some(m) = &self.document.metadata
                && let Ok((mut r, _)) = library.presets[i].apply_lenient(
                    &self.document.edit.recipe,
                    m,
                    &self.document.profiles,
                    self.document
                        .measures()
                        .as_ref()
                        .map(|m| m as &dyn crate::xmp::PhotoMeasures),
                )
            {
                this_photos_upright(&mut r, &self.document.edit.recipe);
                self.presets.preview = Some(r);
                self.schedule();
            }
            // Woken when the hover has lasted long enough to preview; not after,
            // when a preset that cannot apply would otherwise keep it awake.
            if self.presets.preview.is_none()
                && let Some((_, t)) = &self.presets.hover
                && let Some(left) = HOVER_PREVIEW.checked_sub(t.elapsed())
            {
                ui.ctx()
                    .request_repaint_after(left + Duration::from_millis(10));
            }
        } else {
            self.presets.hover = None;
            if self.presets.preview.take().is_some() {
                self.schedule();
            }
        }
    }
}

/// A preset applied to the open photo, as the status bar and the control API report it.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub(super) struct AppliedPreset {
    pub name: String,
    /// The profile used in place of one the preset names but this camera lacks.
    pub substitute: Option<String>,
    /// The settings that do not fit this photo and were left as they were.
    pub skipped: Vec<String>,
}

impl std::fmt::Display for AppliedPreset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Applied {}", self.name)?;
        if let Some(used) = &self.substitute {
            write!(f, " · using {used}")?;
        }
        if !self.skipped.is_empty() {
            write!(f, " · skipped: {}", self.skipped.join("; "))?;
        }
        Ok(())
    }
}

/// Whether an Amount change changed the photo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AmountChange {
    Same,
    Changed,
}

/// Lightroom's preset Amount while it shows: after a preset that supports one is
/// applied, until anything else changes the photo.
pub(super) struct AmountSession {
    name: String,
    amount: f32,
    scale: crate::presets::amount::PresetAmount,
    /// The settings it last set, to notice any other change.
    shown: Recipe,
}

impl AmountSession {
    /// Whether `current` is still what this Amount set. Upright's corrections are
    /// analysed from the photo and land whenever the analysis finishes, so they don't
    /// count; a new Upright mode or guide does.
    fn still_shown(&self, current: &Recipe) -> bool {
        if current.upright.corrections == self.shown.upright.corrections {
            return self.shown == *current;
        }
        let mut shown = self.shown.clone();
        shown
            .upright
            .corrections
            .clone_from(&current.upright.corrections);
        shown == *current
    }
}

impl Editor {
    /// Applies preset `i` of the library to the open photo, as a click does, and
    /// says so in the status bar either way.
    pub(super) fn apply_preset(&mut self, i: usize) -> anyhow::Result<AppliedPreset> {
        let applied = self.apply_library_preset(i);
        self.status = match &applied {
            Ok(applied) => applied.to_string(),
            Err(e) => format!("Preset not applied: {e:#}"),
        };
        applied
    }
    fn apply_library_preset(&mut self, i: usize) -> anyhow::Result<AppliedPreset> {
        self.presets.preview = None;
        self.presets.hover = None;
        self.presets.amount = None;
        let library = self.presets.library.clone();
        let preset = &library.presets[i];
        let m = self
            .document
            .metadata
            .as_ref()
            .context("Open a photo in Develop first")?;
        let (mut r, skipped) = preset.apply_lenient(
            &self.document.edit.recipe,
            m,
            &self.document.profiles,
            self.document
                .measures()
                .as_ref()
                .map(|m| m as &dyn crate::xmp::PhotoMeasures),
        )?;
        let substitute = preset
            .profile_substitute(m, &self.document.profiles)
            .map(|(_, used)| used);
        this_photos_upright(&mut r, &self.document.edit.recipe);
        let before = std::mem::replace(&mut self.document.edit.recipe, r);
        self.ensure_upright();
        // The settings before it are kept once, and every Amount is computed
        // from them again, so dragging never drifts.
        let full = self.document.edit.recipe.clone();
        self.presets.amount =
            crate::presets::amount::PresetAmount::new(preset, before, full.clone())
                .ok()
                .map(|scale| AmountSession {
                    name: crate::presets::display_name(&preset.name),
                    amount: 1.,
                    scale,
                    shown: full,
                });
        self.presets.selected = preset.id.clone();
        Ok(AppliedPreset {
            name: crate::presets::display_name(&preset.name),
            substitute,
            skipped,
        })
    }
    /// Sets the Amount of the preset just applied (0–2, 1 = 100%), and whether that
    /// changed the photo.
    pub(super) fn set_preset_amount(&mut self, amount: f32) -> AmountChange {
        let (Some(session), Some(m)) = (&mut self.presets.amount, &self.document.metadata) else {
            return AmountChange::Same;
        };
        session.amount = amount;
        session.shown = session.scale.at(amount, m);
        // Upright's analysis may have landed since: the photo's.
        session
            .shown
            .upright
            .corrections
            .clone_from(&self.document.edit.recipe.upright.corrections);
        // Named only when it changes the photo: a label left over would name the
        // next edit.
        if session.shown == self.document.edit.recipe {
            // The slider named a step; with nothing changed it would name the next edit.
            self.context
                .data_mut(|d| d.remove_temp::<(String, String)>(super::widgets::history_step_id()));
            return AmountChange::Same;
        }
        self.document.edit.recipe = session.shown.clone();
        self.document.edit.history.label(super::history::Step::new(
            "Preset Amount",
            format!("{:.0}", amount * 100.),
        ));
        AmountChange::Changed
    }
    /// Ends the Amount once anything else has changed the photo, as Lightroom hides it.
    pub(super) fn end_stale_preset_amount(&mut self) {
        if self
            .presets
            .amount
            .as_ref()
            .is_some_and(|s| !s.still_shown(&self.document.edit.recipe))
        {
            self.presets.amount = None;
        }
    }
    /// The Amount slider, as Lightroom shows it at the top of the Presets panel. Any
    /// other change to the photo (an edit, Undo, another preset) ends it; see
    /// `end_stale_preset_amount`, which runs every frame.
    fn preset_amount_ui(&mut self, ui: &mut egui::Ui) {
        let Some(session) = &self.presets.amount else {
            return;
        };
        let mut amount = session.amount;
        ui.add_space(2.);
        ui.add(
            egui::Label::new(
                egui::RichText::new(&session.name)
                    .size(11.)
                    .color(theme::palette(ui.ctx()).gray(170)),
            )
            .truncate(),
        );
        super::widgets::set_edit_context(ui, "Preset");
        ui.push_id("preset-amount", |ui| {
            super::widgets::slider_with(
                ui,
                "Amount",
                &mut amount,
                crate::presets::amount::RANGE,
                1.,
                Some((100., 0)),
                None,
            )
        });
        super::widgets::set_edit_context(ui, "");
        if amount != session.amount {
            self.set_preset_amount(amount);
        }
    }
}

/// The preset list as shown: its groups in order, each with its presets'
/// indices and display names. Building it means formatting and matching
/// every preset, so it is kept until the search, the filters, the library,
/// the favorites or the photo's compatibility change.
pub(super) struct PresetList {
    library: Arc<crate::presets::Library>,
    filter: String,
    favorites_only: bool,
    compatible_only: bool,
    revision: u64,
    groups: Arc<Vec<PresetGroup>>,
}
pub(super) struct PresetGroup {
    imported: bool,
    pub(super) name: String,
    pub(super) presets: Vec<(usize, String)>,
}

impl PresetList {
    /// Whether this list is still the one to show; compares without
    /// allocating, as it runs every frame.
    fn shows(&self, browser: &super::state::PresetBrowser) -> bool {
        Arc::ptr_eq(&self.library, &browser.library)
            && self.filter == browser.filter
            && self.favorites_only == browser.favorites_only
            && self.compatible_only == browser.compatible_only
            && self.revision == browser.revision
    }
    fn build(browser: &super::state::PresetBrowser) -> Self {
        let query = browser.filter.to_lowercase();
        // Built-in groups first, in Lightroom's order, then imported groups
        // by name. A built-in and an imported group of the same name stay
        // apart.
        let mut groups: std::collections::BTreeMap<(bool, usize, String), Vec<(usize, String)>> =
            Default::default();
        for (i, p) in browser.library.presets.iter().enumerate() {
            let issue = browser.issues.get(i).and_then(Option::as_ref);
            if (browser.compatible_only && issue.is_some())
                || (browser.favorites_only && !browser.favorites.contains(&p.id))
                || !crate::presets::display_name(&format!("{} {}", p.group, p.name))
                    .to_lowercase()
                    .contains(&query)
            {
                continue;
            }
            let rank = if p.builtin {
                crate::presets::builtin::group_rank(&p.group)
            } else {
                0
            };
            groups
                .entry((!p.builtin, rank, p.group.clone()))
                .or_default()
                .push((i, crate::presets::display_name(&p.name)));
        }
        Self {
            library: browser.library.clone(),
            filter: browser.filter.clone(),
            favorites_only: browser.favorites_only,
            compatible_only: browser.compatible_only,
            revision: browser.revision,
            groups: Arc::new(
                groups
                    .into_iter()
                    .map(|((imported, _, name), presets)| PresetGroup {
                        imported,
                        name,
                        presets,
                    })
                    .collect(),
            ),
        }
    }
}

impl Editor {
    /// The preset list for the current search and filters; see `PresetList`.
    pub(super) fn preset_list(&mut self) -> Arc<Vec<PresetGroup>> {
        let list = match self.presets.list.take() {
            Some(list) if list.shows(&self.presets) => list,
            _ => PresetList::build(&self.presets),
        };
        let groups = list.groups.clone();
        self.presets.list = Some(list);
        groups
    }
}

/// A preset's tooltip: its name, then whether it fits this photo and how.
fn preset_detail(
    p: &crate::xmp::Preset,
    issue: Option<&String>,
    substitute: Option<&(String, String)>,
) -> String {
    let detail = match issue {
        Some(issue) => format!(
            "{}\nNot fully compatible: {issue}\nClick to apply the supported settings",
            p.name
        ),
        None => format!(
            "{}\nClick to apply · hover to preview\n{}",
            p.name,
            p.notes.join("\n")
        ),
    };
    match substitute {
        Some((asked, used)) => format!(
            "{detail}\nMade for {asked}; renders with {used} because {asked} isn't imported for this camera"
        ),
        None => detail,
    }
}

/// A Lightroom-style list row for preset folders and presets: fixed height,
/// indent, optional disclosure triangle and right-aligned count.
fn list_row(
    ui: &mut egui::Ui,
    text: &str,
    count: Option<usize>,
    depth: usize,
    open: Option<bool>,
    selected: bool,
    enabled: bool,
) -> egui::Response {
    let palette = theme::palette(ui.ctx());
    use egui::{Align2, FontId, Pos2};
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.), Sense::click());
    // Scrolled out of view: keep its place but skip laying out its text.
    if !ui.is_rect_visible(rect) {
        return response;
    }
    if selected || (enabled && response.hovered()) {
        ui.painter().rect_filled(
            rect,
            2.,
            if selected {
                palette.selected_row()
            } else {
                palette.gray(43)
            },
        );
    }
    let x = rect.left() + 8. + depth as f32 * 16.;
    let y = rect.center().y;
    if let Some(open) = open {
        let c = Pos2::new(x + 3., y);
        let triangle = if open {
            vec![
                c + Vec2::new(-4., -2.),
                c + Vec2::new(4., -2.),
                c + Vec2::new(0., 3.),
            ]
        } else {
            vec![
                c + Vec2::new(-2., -4.),
                c + Vec2::new(3., 0.),
                c + Vec2::new(-2., 4.),
            ]
        };
        ui.painter().add(egui::Shape::convex_polygon(
            triangle,
            palette.gray(140),
            Stroke::NONE,
        ));
    }
    let text_left = x + if open.is_some() { 14. } else { 6. };
    let right = rect.right() - if count.is_some() { 36. } else { 26. };
    let galley = egui::WidgetText::from(text).into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        (right - text_left).max(1.),
        FontId::proportional(12.),
    );
    ui.painter().galley(
        Pos2::new(text_left, y - galley.size().y / 2.),
        galley,
        palette.gray(if !enabled {
            95
        } else if selected {
            240
        } else {
            195
        }),
    );
    if let Some(count) = count {
        ui.painter().text(
            Pos2::new(rect.right() - 10., y),
            Align2::RIGHT_CENTER,
            count.to_string(),
            FontId::proportional(10.),
            palette.gray(120),
        );
    }
    response
}

impl Editor {
    /// Lightroom's History panel: this session's steps, newest first, over
    /// the photo's imported Lightroom steps. Clicking a step of this session
    /// goes back (or forward) to it; later steps stay until the next edit.
    fn history_section(&mut self, ui: &mut egui::Ui) {
        if self.document.metadata.is_none() {
            return;
        }
        let mut go_to = None;
        let mut to_before = None;
        let mut lightroom = None;
        let (steps, applied) = self.document.edit.history.steps();
        // A click goes to the step; its menu copies it to Before, as in Lightroom.
        let mut row = |ui: &mut egui::Ui, n: usize, name: &str, value: &str| {
            let response = history_row(ui, name, value, n == applied, n > applied);
            if response.clicked() && !super::widgets::context_clicked(&response) {
                go_to = Some(n);
            }
            super::widgets::context_menu(&response, |ui| {
                ui.set_width(270.);
                if super::widgets::menu_item(
                    ui,
                    "Copy History Step Settings to Before",
                    "",
                    true,
                    false,
                ) {
                    to_before = Some(n);
                    ui.close();
                }
            });
        };
        section(ui, "History", false, |ui| {
            ui.spacing_mut().item_spacing.y = 0.;
            for (i, step) in steps.iter().enumerate().rev() {
                row(ui, i + 1, &step.name, &step.value);
            }
            let opened = if self.document.lightroom_history.is_empty() {
                "Opened"
            } else {
                "Opened with Lightroom edit"
            };
            row(ui, 0, opened, "");
            if self.document.lightroom_history.is_empty() {
                return;
            }
            ui.add_space(10.);
            ui.label(
                egui::RichText::new("From Lightroom")
                    .size(11.)
                    .color(theme::palette(ui.ctx()).gray(125)),
            );
            ui.add_space(4.);
            for (i, step) in self.document.lightroom_history.iter().enumerate().rev() {
                let name = if step.name.is_empty() {
                    "Edit"
                } else {
                    step.name.as_str()
                };
                let response = history_row(ui, name, "", false, false);
                let response = match step.created {
                    // Lightroom counts seconds from 2001-01-01 UTC.
                    Some(s) => response
                        .on_hover_text(format!("{} UTC", format_unix(s as i64 + 978_307_200))),
                    None => response,
                };
                if response.clicked() && !super::widgets::context_clicked(&response) {
                    lightroom = Some((i, LightroomStep::Apply));
                }
                super::widgets::context_menu(&response, |ui| {
                    ui.set_width(270.);
                    if super::widgets::menu_item(
                        ui,
                        "Copy History Step Settings to Before",
                        "",
                        true,
                        false,
                    ) {
                        lightroom = Some((i, LightroomStep::ToBefore));
                        ui.close();
                    }
                });
            }
        });
        if let Some(n) = to_before {
            self.before_from_history(n);
        }
        if let Some(n) = go_to {
            // A resize still being grouped is a step before the jump, so it isn't lost.
            self.finish_wheel_gesture();
            let current = &mut self.document.edit.recipe;
            if self.document.edit.history.jump(n, current) {
                self.ensure_upright();
            }
        }
        if let Some((i, use_step)) = lightroom
            && let Some(m) = &self.document.metadata
        {
            let step = &self.document.lightroom_history[i];
            match crate::lr_develop::convert_develop(
                &step.text,
                m,
                &self.document.profiles,
                self.document
                    .measures()
                    .as_ref()
                    .map(|m| m as &dyn crate::xmp::PhotoMeasures),
            ) {
                Ok((recipe, skipped)) if use_step == LightroomStep::ToBefore => {
                    if !skipped.is_empty() {
                        self.status = format!("Before · not rendered: {}", skipped.join(", "));
                    }
                    self.set_before(recipe);
                }
                Ok((recipe, skipped)) => {
                    let name = format!("Lightroom: {}", step.name);
                    self.status = if skipped.is_empty() {
                        name.clone()
                    } else {
                        format!("{name} · not rendered: {}", skipped.join(", "))
                    };
                    self.document
                        .edit
                        .history
                        .label(super::history::Step::new(name, ""));
                    self.document.edit.recipe = recipe;
                }
                Err(e) => self.status = format!("History step not applied: {e:#}"),
            }
        }
    }
}
/// What a click on a Lightroom History step asked for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LightroomStep {
    /// Apply it to the edit, as a step.
    Apply,
    /// Copy History Step Settings to Before.
    ToBefore,
}
/// A preset's Upright mode, with this photo's own corrections rather than any the preset
/// carries from the photo it was saved from; a mode without one is analysed on apply.
/// New lens settings call for a new analysis, as pasting them does.
fn this_photos_upright(r: &mut Recipe, current: &Recipe) {
    if r.upright != current.upright {
        let mode = r.upright.mode;
        r.upright.clone_from(&current.upright);
        r.upright.mode = mode;
        if mode == crate::model::transform::UprightMode::Guided && r.upright.correction().is_none()
        {
            r.upright.mode = current.upright.mode;
        }
    }
    use crate::model::transform::LensInputs;
    if LensInputs::of(r) != LensInputs::of(current) {
        r.upright.analyse_again();
    }
}
/// A History row: the step on the left, its value on the right. The current
/// step is highlighted; steps after it (undone) are dimmed, as in Lightroom.
fn history_row(
    ui: &mut egui::Ui,
    name: &str,
    value: &str,
    current: bool,
    undone: bool,
) -> egui::Response {
    let palette = theme::palette(ui.ctx());
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 24.), Sense::click());
    if current {
        ui.painter().rect_filled(rect, 3., palette.accent());
    } else if response.hovered() {
        ui.painter().rect_filled(rect, 3., palette.gray(43));
    }
    let color = if current {
        palette.on_accent_text(250)
    } else {
        palette.gray(if undone { 120 } else { 205 })
    };
    let value_width = if value.is_empty() {
        0.
    } else {
        let galley =
            ui.painter()
                .layout_no_wrap(value.to_string(), egui::FontId::proportional(12.), color);
        let width = galley.size().x;
        ui.painter().galley(
            egui::pos2(
                rect.right() - 8. - width,
                rect.center().y - galley.size().y / 2.,
            ),
            galley,
            color,
        );
        width + 16.
    };
    let clip = egui::Rect::from_min_max(
        rect.min,
        egui::pos2(rect.right() - 8. - value_width, rect.bottom()),
    );
    ui.painter().with_clip_rect(clip).text(
        rect.left_center() + Vec2::new(8., 0.),
        egui::Align2::LEFT_CENTER,
        name,
        egui::FontId::proportional(12.),
        color,
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}
/// "2016-07-28 06:14" from Unix seconds.
fn format_unix(seconds: i64) -> String {
    let [year, month, day, hour, minute, _] = crate::time::utc(seconds);
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}")
}
#[cfg(test)]
#[test]
fn formats_lightroom_history_dates() {
    assert_eq!(format_unix(1_469_686_444), "2016-07-28 06:14");
}

#[cfg(test)]
mod tests {
    use super::*;
    /// A preset that changes only a lens setting leaves no Upright correction analysed
    /// through the old one: the editor analyses the photo again.
    #[test]
    fn presets_with_new_lens_settings_drop_the_photos_upright_analysis() {
        let mut current = Recipe::default();
        current.upright.mode = crate::model::transform::UprightMode::Level;
        current.upright.corrections = vec![[1., 0., 0., 0., 1., 0., 0., 0., 1.]; 4];
        let mut same = current.clone();
        same.exposure = 1.;
        this_photos_upright(&mut same, &current);
        assert_eq!(same.upright, current.upright);
        let mut lens = current.clone();
        lens.lens_manual_distortion = 0.3;
        this_photos_upright(&mut lens, &current);
        assert!(lens.upright.corrections.is_empty());
        assert_eq!(
            lens.upright.mode,
            crate::model::transform::UprightMode::Level
        );
    }
}
