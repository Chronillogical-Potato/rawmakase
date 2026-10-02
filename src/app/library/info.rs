//! The Library's right panel: Quick Develop and the selected photo's metadata.
use super::*;

impl Library {
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
                    metadata_row(ui, "File Name", &field(|p| &p.filename));
                    match photo.as_ref().filter(|p| p.master.is_some()) {
                        Some(p) => {
                            match self.copy_names.row(ui, p, &self.catalog, &mut self.photos) {
                                Ok(true) => self.filter(),
                                Ok(false) => {}
                                Err(e) => {
                                    self.message = format!("Copy name could not be saved: {e}")
                                }
                            }
                        }
                        None => {
                            metadata_row(ui, "Copy Name", "");
                        }
                    }
                    for (key, value, hover) in [
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
        theme::gray(135),
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
        theme::gray(if value.is_empty() { 90 } else { 205 }),
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
        theme::gray(150),
    );
}
