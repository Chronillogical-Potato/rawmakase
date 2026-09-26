//! Catalog browsing; thumbnail work is bounded and independent of RAW development.
use super::widgets::{section, segmented};
use crate::catalog::{Catalog, Collection, Folder, Photo};
use anyhow::Result;
use eframe::egui::{self, Color32, Vec2};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
    sync::mpsc::{Receiver, SyncSender},
};

pub enum Action {
    None,
    Develop(i64),
    RelinkRoot(i64),
    RelinkFolder(i64),
    AddFolder,
}
pub struct Library {
    pub catalog: Catalog,
    pub photos: Vec<Photo>,
    pub selected: Option<i64>,
    folders: Vec<Folder>,
    collections: Vec<Collection>,
    roots: Vec<(i64, String, Option<String>)>,
    folder_scope: Option<HashSet<i64>>,
    selected_folder: String,
    expanded: HashSet<String>,
    collection: Option<i64>,
    members: HashSet<i64>,
    query: String,
    rating: i32,
    flag: i32,
    label_filter: Option<String>,
    only_missing: bool,
    reverse: bool,
    thumb_size: f32,
    strip_current: Option<i64>,
    visible: Vec<usize>,
    available: HashSet<PathBuf>,
    thumbs: HashMap<PathBuf, egui::TextureHandle>,
    thumb_order: VecDeque<PathBuf>,
    pending: HashSet<PathBuf>,
    failed: HashSet<PathBuf>,
    thumb_tx: SyncSender<PathBuf>,
    thumb_rx: Receiver<previews::PreviewResult>,
    preview_progress: previews::Progress,
    pub message: String,
}
impl Library {
    pub fn load(path: &std::path::Path, ctx: egui::Context) -> Result<Self> {
        crate::platform::network::prepare_filesystem_bridge();
        let catalog = Catalog::open(path)?;
        let (tx, result_rx) =
            previews::spawn(crate::catalog::preview_cache::PreviewCache::path(), ctx);
        let mut s = Self {
            catalog,
            photos: Vec::new(),
            selected: None,
            folders: Vec::new(),
            collections: Vec::new(),
            roots: Vec::new(),
            folder_scope: None,
            selected_folder: String::new(),
            expanded: HashSet::new(),
            collection: None,
            members: HashSet::new(),
            query: String::new(),
            rating: 0,
            flag: 2,
            label_filter: None,
            only_missing: false,
            reverse: false,
            thumb_size: 190.,
            strip_current: None,
            visible: Vec::new(),
            available: HashSet::new(),
            thumbs: HashMap::new(),
            thumb_order: VecDeque::new(),
            pending: HashSet::new(),
            failed: HashSet::new(),
            thumb_tx: tx,
            thumb_rx: result_rx,
            preview_progress: Default::default(),
            message: String::new(),
        };
        s.refresh()?;
        // Start with a selection, as Lightroom does, so the side panels are filled.
        s.selected = s.visible.first().map(|i| s.photos[*i].id);
        Ok(s)
    }
    pub fn refresh(&mut self) -> Result<()> {
        self.photos = self.catalog.photos()?;
        self.folders = self.catalog.folders()?;
        self.collections = self.catalog.collections()?;
        self.roots = self.catalog.roots()?;
        self.available = available_paths(&self.photos);
        self.failed.clear();
        self.filter();
        Ok(())
    }
    pub fn available_count(&self) -> usize {
        self.photos
            .iter()
            .filter(|p| self.available.contains(&p.path))
            .count()
    }
    fn filter(&mut self) {
        let q = self.query.to_lowercase();
        self.visible = self
            .photos
            .iter()
            .enumerate()
            .filter(|(_, p)| {
                self.folder_scope
                    .as_ref()
                    .is_none_or(|ids| ids.contains(&p.folder))
                    && (self.collection.is_none() || self.members.contains(&p.id))
                    && self
                        .label_filter
                        .as_ref()
                        .is_none_or(|label| &p.label == label)
                    && p.rating >= self.rating
                    && (self.flag == 2 || p.flag == self.flag)
                    && (!self.only_missing || !self.available.contains(&p.path))
                    && (q.is_empty()
                        || format!(
                            "{} {} {} {} {}",
                            p.filename, p.copy_name, p.keywords, p.captured, p.label
                        )
                        .to_lowercase()
                        .contains(&q))
            })
            .map(|(i, _)| i)
            .collect();
        if self.reverse {
            self.visible.reverse()
        }
        if self
            .selected
            .is_some_and(|id| !self.visible.iter().any(|i| self.photos[*i].id == id))
        {
            self.selected = None;
        }
    }
    pub fn photo(&self, id: i64) -> Option<&Photo> {
        self.photos.iter().find(|p| p.id == id)
    }
    pub fn navigate(&self, id: i64, delta: i32) -> Option<i64> {
        let at = self.visible.iter().position(|i| self.photos[*i].id == id)?;
        let n = (at as i32 + delta).clamp(0, self.visible.len().saturating_sub(1) as i32) as usize;
        self.visible.get(n).map(|i| self.photos[*i].id)
    }
    fn labels(&self) -> Vec<String> {
        let mut labels: Vec<String> = crate::app::photo_metadata::LABELS
            .iter()
            .map(|s| (*s).into())
            .collect();
        let mut custom: Vec<_> = self
            .photos
            .iter()
            .map(|p| &p.label)
            .filter(|s| !s.is_empty() && !labels.contains(s))
            .cloned()
            .collect();
        custom.sort();
        custom.dedup();
        labels.extend(custom);
        labels
    }
    pub fn edit_metadata(
        &mut self,
        id: i64,
        edit: crate::app::photo_metadata::Edit,
        advance: bool,
    ) -> Result<Option<i64>> {
        let p = self
            .photo(id)
            .ok_or_else(|| anyhow::anyhow!("Unknown photo"))?;
        let (rating, flag, label) = edit.values(p);
        let position = self.visible.iter().position(|i| self.photos[*i].id == id);
        let following: Vec<_> = position.map_or_else(Vec::new, |at| {
            self.visible[at + 1..]
                .iter()
                .map(|i| self.photos[*i].id)
                .collect()
        });
        self.catalog.set_metadata(id, rating, flag, &label)?;
        let p = self.photos.iter_mut().find(|p| p.id == id).unwrap();
        p.rating = rating;
        p.flag = flag;
        p.label = label;
        self.message = format!(
            "{} · {} stars · {} · {}",
            p.filename,
            rating,
            match flag {
                1 => "Pick",
                -1 => "Reject",
                _ => "Unflagged",
            },
            if p.label.is_empty() {
                "No label"
            } else {
                &p.label
            }
        );
        self.filter();
        let next = following
            .into_iter()
            .find(|next| self.visible.iter().any(|i| self.photos[*i].id == *next));
        let still_visible = self.visible.iter().any(|i| self.photos[*i].id == id);
        if advance || !still_visible {
            self.selected = next.or_else(|| {
                if still_visible {
                    Some(id)
                } else {
                    self.visible.last().map(|i| self.photos[*i].id)
                }
            });
        }
        Ok(if advance { next } else { None })
    }
    pub fn metadata_controls(&mut self, ui: &mut egui::Ui, id: i64) -> bool {
        if let Some(photo) = self.photo(id).cloned()
            && let Some(edit) = crate::app::photo_metadata::controls(ui, &photo, &self.labels())
        {
            if let Err(e) = self.edit_metadata(id, edit, false) {
                self.message = format!("Metadata could not be saved: {e}");
            }
            return true;
        }
        false
    }
    pub fn sidebar(&mut self, ui: &mut egui::Ui) -> Action {
        let mut action = Action::None;
        ui.spacing_mut().item_spacing.y = 0.;
        egui::ScrollArea::vertical()
            .id_salt("library-sources")
            .show(ui, |ui| {
                section(ui, "Navigator", false, |ui| {
                    let texture = self
                        .selected
                        .and_then(|id| self.photo(id))
                        .and_then(|p| self.thumbs.get(&p.path));
                    let (rect, _) = ui.allocate_exact_size(
                        Vec2::new(ui.available_width(), ui.available_width() * 0.66),
                        egui::Sense::hover(),
                    );
                    ui.painter().rect_filled(rect, 0., Color32::from_gray(22));
                    if texture.is_none() {
                        ui.painter().text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            "No photo selected",
                            egui::FontId::proportional(11.),
                            Color32::from_gray(95),
                        );
                    }
                    if let Some(texture) = texture {
                        let size = texture.size_vec2();
                        let scale = (rect.width() / size.x).min(rect.height() / size.y);
                        ui.painter().image(
                            texture.id(),
                            egui::Rect::from_center_size(rect.center(), size * scale),
                            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.)),
                            Color32::WHITE,
                        );
                    }
                });
                section(ui, "Catalog", false, |ui| {
                    let offline = self.photos.len() - self.available_count();
                    let all = self.folder_scope.is_none()
                        && self.collection.is_none()
                        && !self.only_missing;
                    if source_row(ui, "All Photographs", self.photos.len(), all).clicked() {
                        self.folder_scope = None;
                        self.selected_folder.clear();
                        self.collection = None;
                        self.only_missing = false;
                        self.filter()
                    }
                    if offline > 0
                        && source_row(ui, "Offline Photographs", offline, self.only_missing)
                            .clicked()
                    {
                        self.only_missing = !self.only_missing;
                        self.filter()
                    }
                });
                section(ui, "Folders", false, |ui| {
                    for (root, original, mapped) in self.roots.clone() {
                        let root_path = mapped.as_deref().unwrap_or(&original);
                        let name = std::path::Path::new(&original)
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_string();
                        let mut tree = FolderNode::root(root, name, root_path.into());
                        for f in self.folders.iter().filter(|f| f.root == root) {
                            tree.insert(f);
                        }
                        tree.finish();
                        match folder_tree_row(
                            ui,
                            &tree,
                            0,
                            &mut self.expanded,
                            &self.selected_folder,
                        ) {
                            Some(TreeAction::Select(key, ids)) => {
                                self.selected_folder = key;
                                self.folder_scope = Some(ids);
                                self.collection = None;
                                self.filter();
                            }
                            Some(TreeAction::Relink(root, id)) => {
                                action = if root {
                                    Action::RelinkRoot(id)
                                } else {
                                    Action::RelinkFolder(id)
                                }
                            }
                            None => {}
                        }
                    }
                    if self.roots.is_empty() {
                        ui.add_space(4.);
                        ui.label(
                            egui::RichText::new("No folders yet")
                                .size(11.)
                                .color(Color32::from_gray(120)),
                        );
                    }
                    ui.add_space(12.);
                    if add_row(ui, "Add Folder…")
                        .on_hover_text(
                            "Add a folder of photos to this catalog. Photos stay where they are.",
                        )
                        .clicked()
                    {
                        action = Action::AddFolder;
                    }
                });
            });
        action
    }
    /// Right panel: the selected photo's rating, flag, label and file details.
    /// Right panel: the selected photo's rating, flag, label and file details.
    /// The layout is identical with or without a selection, so nothing moves.
    pub fn info_panel(&mut self, ui: &mut egui::Ui) -> Action {
        let mut action = Action::None;
        ui.spacing_mut().item_spacing.y = 0.;
        let photo = self.selected.and_then(|id| self.photo(id)).cloned();
        egui::ScrollArea::vertical()
            .id_salt("library-info")
            .auto_shrink(false)
            .show(ui, |ui| {
                section(ui, "Quick Develop", false, |ui| {
                    let open = ui
                        .add_enabled(
                            photo.is_some(),
                            egui::Button::new("Open in Develop")
                                .min_size(Vec2::new(ui.available_width(), 24.)),
                        )
                        .on_hover_text("Develop · D, or double-click the photo");
                    if open.clicked()
                        && let Some(p) = &photo
                    {
                        action = Action::Develop(p.id);
                    }
                    ui.add_space(4.);
                    info_text(
                        ui,
                        if photo.as_ref().is_some_and(|p| p.has_lightroom_edits) {
                            "Has Lightroom edits"
                        } else {
                            ""
                        },
                    );
                });
                section(ui, "Metadata", false, |ui| {
                    match &photo {
                        Some(p) => {
                            self.metadata_controls(ui, p.id);
                        }
                        None => {
                            ui.allocate_exact_size(
                                Vec2::new(ui.available_width(), 22.),
                                egui::Sense::hover(),
                            );
                        }
                    }
                    ui.add_space(6.);
                    let folder = photo
                        .as_ref()
                        .and_then(|p| p.path.parent()?.file_name())
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let field = |f: fn(&Photo) -> &str| photo.as_ref().map_or("", f).to_string();
                    for (key, value, hover) in [
                        ("File Name", field(|p| &p.filename), None),
                        ("Copy Name", field(|p| &p.copy_name), None),
                        (
                            "Folder",
                            folder,
                            photo.as_ref().map(|p| p.path.display().to_string()),
                        ),
                        ("Capture Time", field(|p| &p.captured), None),
                        ("Format", field(|p| &p.format), None),
                    ] {
                        let response = metadata_row(ui, key, &value);
                        if let Some(hover) = hover {
                            response.on_hover_text(hover);
                        } else if !value.is_empty() {
                            response.on_hover_text(value);
                        }
                    }
                });
                section(ui, "Keywording", false, |ui| {
                    info_text(
                        ui,
                        match &photo {
                            Some(p) if !p.keywords.is_empty() => &p.keywords,
                            Some(_) => "No keywords",
                            None => "",
                        },
                    );
                });
            });
        action
    }
    /// Drain in every workspace so the bounded worker never waits for the grid.
    pub(super) fn poll_previews(&mut self, ctx: &egui::Context) {
        while let Ok(result) = self.thumb_rx.try_recv() {
            self.preview_progress.finish(&result);
            let previews::PreviewResult {
                path, image: im, ..
            } = result;
            self.pending.remove(&path);
            if let Some(im) = im {
                while self.thumbs.len() >= 192 {
                    if let Some(old) = self.thumb_order.pop_front() {
                        self.thumbs.remove(&old);
                    } else {
                        break;
                    }
                }
                self.thumbs.insert(
                    path.clone(),
                    ctx.load_texture(
                        path.display().to_string(),
                        egui::ColorImage::from_rgb(
                            [im.width() as usize, im.height() as usize],
                            im.as_raw(),
                        ),
                        egui::TextureOptions::LINEAR,
                    ),
                );
                self.thumb_order.push_back(path);
            } else {
                self.failed.insert(path);
            }
        }
    }
    /// The Library's cached preview for a photo, if one is loaded.
    pub(super) fn thumbnail(&self, path: &std::path::Path) -> Option<&egui::TextureHandle> {
        self.thumbs.get(path)
    }
    pub(super) fn preview_progress_active(&self) -> bool {
        self.preview_progress.active()
    }
    pub(super) fn preview_progress(&self, ui: &mut egui::Ui) {
        self.preview_progress.show(ui);
    }
    fn filter_bar(&mut self, ui: &mut egui::Ui) {
        use crate::app::photo_metadata::{LABELS, label_color};
        let mut changed = false;
        egui::Frame::new()
            .fill(Color32::from_gray(38))
            .inner_margin(egui::Margin::symmetric(10, 6))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.;
                    ui.spacing_mut().interact_size.y = 20.;
                    let compact = ui.available_width() < 640.;
                    changed |= ui
                        .add(
                            egui::TextEdit::singleline(&mut self.query)
                                .hint_text("Search")
                                .font(egui::FontId::proportional(12.))
                                .desired_width(if compact { 120. } else { 190. }),
                        )
                        .on_hover_text("Search filename, keyword, capture date or label")
                        .changed();
                    ui.add_space(10.);
                    if !compact {
                        ui.label(filter_caption("Flag"));
                    }
                    changed |= segmented(
                        ui,
                        &mut self.flag,
                        &[
                            (2, "All"),
                            (1, "Picked"),
                            (0, "Unflagged"),
                            (-1, "Rejected"),
                        ],
                        if compact { 190. } else { 230. },
                    );
                    ui.add_space(10.);
                    if !compact {
                        ui.label(filter_caption("Rating ≥"));
                    }
                    for star in 1..=5 {
                        let (rect, response) =
                            ui.allocate_exact_size(Vec2::new(13., 20.), egui::Sense::click());
                        let lit = self.rating >= star;
                        ui.painter().text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            "★",
                            egui::FontId::proportional(12.),
                            Color32::from_gray(if lit {
                                225
                            } else if response.hovered() {
                                140
                            } else {
                                80
                            }),
                        );
                        if response
                            .on_hover_text(format!("{star} stars or more · click again to clear"))
                            .clicked()
                        {
                            self.rating = if self.rating == star { 0 } else { star };
                            changed = true;
                        }
                    }
                    ui.add_space(10.);
                    if !compact {
                        ui.label(filter_caption("Color"));
                    }
                    for label in LABELS {
                        let active = self.label_filter.as_deref() == Some(label);
                        let (rect, response) =
                            ui.allocate_exact_size(Vec2::new(15., 20.), egui::Sense::click());
                        let chip = egui::Rect::from_center_size(rect.center(), Vec2::splat(10.));
                        ui.painter()
                            .rect_filled(chip, 1., label_color(label).unwrap_or_default());
                        if active {
                            ui.painter().rect_stroke(
                                chip.expand(2.),
                                2.,
                                egui::Stroke::new(1.2, Color32::from_gray(225)),
                                egui::StrokeKind::Outside,
                            );
                        }
                        if response.on_hover_text(label).clicked() {
                            self.label_filter = if active { None } else { Some(label.into()) };
                            changed = true;
                        }
                    }
                    let custom: Vec<_> = self
                        .labels()
                        .into_iter()
                        .filter(|l| !LABELS.contains(&l.as_str()))
                        .collect();
                    if !custom.is_empty() {
                        egui::ComboBox::from_id_salt("library-label")
                            .width(70.)
                            .selected_text(
                                self.label_filter
                                    .as_deref()
                                    .filter(|l| custom.iter().any(|c| c == l))
                                    .unwrap_or("Other"),
                            )
                            .show_ui(ui, |ui| {
                                for label in custom {
                                    changed |= ui
                                        .selectable_value(
                                            &mut self.label_filter,
                                            Some(label.clone()),
                                            label,
                                        )
                                        .changed();
                                }
                            });
                    }
                    let active = !self.query.is_empty()
                        || self.rating > 0
                        || self.flag != 2
                        || self.label_filter.is_some();
                    if active {
                        ui.add_space(10.);
                        if ui
                            .add(
                                egui::Button::new(filter_caption("Filters Off"))
                                    .small()
                                    .frame(false),
                            )
                            .on_hover_text("Clear search, flag, rating and color filters")
                            .clicked()
                        {
                            self.query.clear();
                            self.rating = 0;
                            self.flag = 2;
                            self.label_filter = None;
                            changed = true;
                        }
                    }
                });
            });
        if changed {
            self.filter()
        }
    }
    fn grid_toolbar(&mut self, ui: &mut egui::Ui) {
        egui::Frame::new()
            .fill(Color32::from_gray(38))
            .inner_margin(egui::Margin::symmetric(10, 4))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(filter_caption("Sort"));
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new(if self.reverse {
                                    "Capture Time ↓"
                                } else {
                                    "Capture Time ↑"
                                })
                                .size(11.),
                            )
                            .small()
                            .frame(false),
                        )
                        .on_hover_text("Reverse the sort order")
                        .clicked()
                    {
                        self.reverse = !self.reverse;
                        self.filter();
                    }
                    ui.add_space(12.);
                    ui.small(if self.visible.len() == self.photos.len() {
                        format!("{} photos", self.photos.len())
                    } else {
                        format!("{} of {} photos", self.visible.len(), self.photos.len())
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().slider_width = 110.;
                        ui.add(
                            egui::Slider::new(&mut self.thumb_size, 110. ..=360.).show_value(false),
                        );
                        ui.label(filter_caption("Thumbnails"));
                    });
                });
            });
    }
    fn request_thumbnail(&mut self, path: &std::path::Path, ctx: &egui::Context) {
        if !self.thumbs.contains_key(path)
            && !self.pending.contains(path)
            && !self.failed.contains(path)
            && self.thumb_tx.try_send(path.to_path_buf()).is_ok()
        {
            self.pending.insert(path.to_path_buf());
            self.preview_progress.queued();
            ctx.request_repaint();
        }
    }
    /// The selected source's name, as Lightroom shows it above the filmstrip.
    fn source_name(&self) -> String {
        if let Some(id) = self.collection {
            return self
                .collections
                .iter()
                .find(|c| c.id == id)
                .map_or_else(|| "Collection".into(), |c| c.name.clone());
        }
        if self.selected_folder.is_empty() {
            return "All Photographs".into();
        }
        match self.selected_folder.rsplit_once('/') {
            Some((_, name)) => name.into(),
            None => self
                .roots
                .iter()
                .find(|(id, _, _)| format!("root:{id}") == self.selected_folder)
                .and_then(|(_, original, _)| {
                    std::path::Path::new(original)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                })
                .unwrap_or_default(),
        }
    }
    /// Lightroom's filmstrip for Develop: the current source's photos with the
    /// open one highlighted. Returns a photo to open and whether metadata changed.
    pub fn filmstrip(&mut self, ui: &mut egui::Ui, current: i64) -> (Option<i64>, bool) {
        let mut target = None;
        let mut changed = false;
        let photo = self.photo(current).cloned();
        let position = self
            .visible
            .iter()
            .position(|i| self.photos[*i].id == current);
        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(10, 3))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(self.source_name())
                            .size(11.)
                            .color(Color32::from_gray(200)),
                    );
                    ui.label(filter_caption(&match position {
                        Some(at) => format!("{} of {} photos", at + 1, self.visible.len()),
                        None => format!("{} photos", self.visible.len()),
                    }));
                    if let Some(p) = &photo {
                        ui.label(filter_caption(&p.filename));
                    }
                    if photo.is_some() {
                        ui.add_space((ui.available_width() - 250.).max(8.));
                        changed = self.metadata_controls(ui, current);
                    }
                });
            });
        let height = ui.available_height().max(40.);
        egui::ScrollArea::horizontal()
            .id_salt("develop-filmstrip")
            .auto_shrink(false)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.;
                    for n in 0..self.visible.len() {
                        let p = self.photos[self.visible[n]].clone();
                        let (rect, response) = ui.allocate_exact_size(
                            Vec2::new(height * 1.25, height),
                            egui::Sense::click(),
                        );
                        let active = p.id == current;
                        if active && self.strip_current != Some(current) {
                            response.scroll_to_me(Some(egui::Align::Center));
                            self.strip_current = Some(current);
                        }
                        if !ui.is_rect_visible(rect) {
                            continue;
                        }
                        self.request_thumbnail(&p.path, ui.ctx());
                        let cell = rect.shrink(2.);
                        let base = Color32::from_gray(if active {
                            120
                        } else if response.hovered() {
                            58
                        } else {
                            40
                        });
                        // Same cues as the grid: the label tints the cell, and
                        // flag, stars and label chip sit on a strip below the photo.
                        let fill = crate::app::photo_metadata::label_color(&p.label)
                            .map_or(base, |label| {
                                base.lerp_to_gamma(label, if active { 0.35 } else { 0.25 })
                            });
                        ui.painter().rect_filled(cell, 2., fill);
                        let strip = 14.;
                        if let Some(texture) = self.thumbs.get(&p.path) {
                            let area = egui::Rect::from_min_max(
                                cell.min + Vec2::splat(5.),
                                cell.max - Vec2::new(5., strip + 2.),
                            );
                            let size = texture.size_vec2();
                            let scale = (area.width() / size.x).min(area.height() / size.y);
                            ui.painter().image(
                                texture.id(),
                                egui::Rect::from_center_size(area.center(), size * scale),
                                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.)),
                                Color32::WHITE,
                            );
                        }
                        let y = cell.bottom() - strip / 2. - 2.;
                        let mut x = cell.left() + 6.;
                        if p.flag != 0 {
                            crate::app::photo_metadata::flag_icon(
                                ui.painter(),
                                egui::pos2(x + 4., y),
                                p.flag,
                                active,
                            );
                            x += 13.;
                        }
                        if p.rating > 0 {
                            ui.painter().text(
                                egui::pos2(x, y),
                                egui::Align2::LEFT_CENTER,
                                "★".repeat(p.rating as usize),
                                egui::FontId::proportional(9.),
                                Color32::from_gray(if active { 30 } else { 200 }),
                            );
                        }
                        if let Some(color) = crate::app::photo_metadata::label_color(&p.label) {
                            ui.painter().rect_filled(
                                egui::Rect::from_center_size(
                                    egui::pos2(cell.right() - 10., y),
                                    Vec2::splat(8.),
                                ),
                                1.,
                                color,
                            );
                        }
                        if let Some(menu) = cell::photo_menu(&response, &p) {
                            let is_edit = matches!(menu, cell::PhotoAction::Edit(_));
                            if let Some(id) = self.photo_action(ui.ctx(), &p, menu) {
                                target = Some(id).filter(|id| *id != current);
                            }
                            if is_edit {
                                // Filters may have changed the visible list.
                                changed = true;
                                break;
                            }
                        }
                        let context = crate::app::widgets::context_clicked(&response);
                        if response.on_hover_text(&p.filename).clicked() && !active && !context {
                            target = Some(p.id);
                        }
                    }
                });
            });
        (target, changed)
    }
    pub fn grid(&mut self, ui: &mut egui::Ui) -> Action {
        self.poll_previews(ui.ctx());
        let mut action = Action::None;
        self.filter_bar(ui);
        egui::Panel::bottom("library-grid-toolbar")
            .frame(egui::Frame::new())
            .show_separator_line(false)
            .show(ui, |ui| self.grid_toolbar(ui));
        let columns = ((ui.available_width() / self.thumb_size).floor() as usize).max(1);
        let width = (ui.available_width() / columns as f32).floor().max(80.);
        let mut metadata_edit = None;
        let spacing = ui.spacing().item_spacing;
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        egui::Frame::new()
            .fill(Color32::from_gray(44))
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("library-grid")
                    .auto_shrink(false)
                    .show_rows(
                        ui,
                        width,
                        self.visible.len().div_ceil(columns),
                        |ui, rows| {
                            for row in rows {
                                ui.horizontal(|ui| {
                                    for col in 0..columns {
                                        let Some(&index) = self.visible.get(row * columns + col)
                                        else {
                                            break;
                                        };
                                        let p = self.photos[index].clone();
                                        let exists = self.available.contains(&p.path);
                                        self.request_thumbnail(&p.path, ui.ctx());
                                        let (response, edit) = photo_cell(
                                            ui,
                                            &p,
                                            self.thumbs.get(&p.path),
                                            self.selected == Some(p.id),
                                            row * columns + col + 1,
                                            exists,
                                            width,
                                        );
                                        if response.clicked() || response.secondary_clicked() {
                                            self.selected = Some(p.id);
                                        }
                                        if response.double_clicked() {
                                            self.selected = Some(p.id);
                                            action = Action::Develop(p.id);
                                        }
                                        if let Some(edit) = edit {
                                            metadata_edit = Some((p.clone(), edit));
                                        }
                                    }
                                });
                            }
                        },
                    );
            });
        ui.spacing_mut().item_spacing = spacing;
        if let Some((photo, menu)) = metadata_edit
            && let Some(id) = self.photo_action(ui.ctx(), &photo, menu)
        {
            action = Action::Develop(id);
        }
        action
    }
    /// Carries out a thumbnail menu choice; returns a photo to open in Develop.
    fn photo_action(
        &mut self,
        ctx: &egui::Context,
        photo: &Photo,
        action: cell::PhotoAction,
    ) -> Option<i64> {
        use cell::PhotoAction;
        match action {
            PhotoAction::Develop => return Some(photo.id),
            PhotoAction::Reveal => {
                if let Err(e) = crate::platform::reveal::reveal(&photo.path) {
                    self.message = format!("Could not show {}: {e}", photo.filename);
                }
            }
            PhotoAction::CopyPath => {
                ctx.copy_text(photo.path.display().to_string());
                self.message = format!("Copied {}", photo.path.display());
            }
            PhotoAction::Edit(edit) => {
                if let Err(e) = self.edit_metadata(photo.id, edit, false) {
                    self.message = format!("Metadata could not be saved: {e}");
                }
            }
        }
        None
    }
}
/// A fixed-height metadata row: caption column, then the truncated value
/// (a dash when empty), so the panel never widens or reflows.
fn metadata_row(ui: &mut egui::Ui, key: &str, value: &str) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 20.), egui::Sense::hover());
    let y = rect.center().y;
    ui.painter().text(
        egui::pos2(rect.left() + 84., y),
        egui::Align2::RIGHT_CENTER,
        key,
        egui::FontId::proportional(11.),
        Color32::from_gray(135),
    );
    let left = rect.left() + 92.;
    let galley = egui::WidgetText::from(if value.is_empty() { "—" } else { value }).into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        (rect.right() - left).max(1.),
        egui::FontId::proportional(11.),
    );
    ui.painter().galley(
        egui::pos2(left, y - galley.size().y / 2.),
        galley,
        Color32::from_gray(if value.is_empty() { 90 } else { 205 }),
    );
    response
}
/// One truncated line of secondary text at a fixed height.
fn info_text(ui: &mut egui::Ui, text: &str) {
    let (rect, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 18.), egui::Sense::hover());
    let galley = egui::WidgetText::from(text).into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        rect.width().max(1.),
        egui::FontId::proportional(11.),
    );
    ui.painter().galley(
        egui::pos2(rect.left(), rect.center().y - galley.size().y / 2.),
        galley,
        Color32::from_gray(150),
    );
}
fn filter_caption(text: &str) -> egui::RichText {
    egui::RichText::new(text)
        .size(11.)
        .color(Color32::from_gray(150))
}
/// A quiet full-width "+ label" row, Lightroom's add action in a panel.
fn add_row(ui: &mut egui::Ui, label: &str) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 24.), egui::Sense::click());
    let hovered = response.hovered();
    if hovered {
        ui.painter().rect_filled(rect, 3., Color32::from_gray(43));
    }
    let color = Color32::from_gray(if hovered { 235 } else { 165 });
    // Same columns as folder rows: icon at 10 px, text at 29 px.
    let c = egui::pos2(rect.left() + 16., rect.center().y);
    let stroke = egui::Stroke::new(1.4, color);
    ui.painter()
        .line_segment([c - Vec2::new(5., 0.), c + Vec2::new(5., 0.)], stroke);
    ui.painter()
        .line_segment([c - Vec2::new(0., 5.), c + Vec2::new(0., 5.)], stroke);
    ui.painter().text(
        egui::pos2(rect.left() + 29., rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(12.),
        color,
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}
/// A Catalog panel row: name on the left, photo count right-aligned.
fn source_row(ui: &mut egui::Ui, name: &str, count: usize, active: bool) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.), egui::Sense::click());
    if active || response.hovered() {
        ui.painter().rect_filled(
            rect,
            2.,
            if active {
                Color32::from_rgb(47, 58, 66)
            } else {
                Color32::from_gray(43)
            },
        );
    }
    let y = rect.center().y;
    ui.painter().text(
        egui::pos2(rect.left() + 10., y),
        egui::Align2::LEFT_CENTER,
        name,
        egui::FontId::proportional(12.),
        Color32::from_gray(if active { 235 } else { 190 }),
    );
    ui.painter().text(
        egui::pos2(rect.right() - 10., y),
        egui::Align2::RIGHT_CENTER,
        count.to_string(),
        egui::FontId::proportional(11.),
        Color32::from_gray(125),
    );
    response
}
/// Lightroom-style grid cells: the label tints the cell, while selection uses
/// a lighter surround instead of the app's blue button fill.
mod cell;
mod previews;
mod thumbnails;
mod tree;
use cell::photo_cell;
use thumbnails::{available_paths, thumbnail};
use tree::{FolderNode, TreeAction, folder_tree_row};
#[cfg(test)]
mod tests;
