//! Lightroom's Watermark Editor, as a sheet over the Export dialog: the open
//! photo with the mark on it, and the mark's style, text or image options,
//! shadow and effects. Watermarks are kept as presets, a graphic one with
//! its own copy of the image.
use super::super::widgets::{modal_frame, primary_button};
use super::Editor;
use crate::app::theme;
use crate::watermark::{self, Align, Anchor, Size, Style, Watermark};
use eframe::egui::{self, Color32, Sense, Vec2};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

const WIDTH: f32 = 920.;
const HEIGHT: f32 = 620.;
const PANEL: f32 = 300.;

/// The editor's state while it is open.
pub(super) struct WatermarkEditor {
    pub(super) watermark: Watermark,
    /// The saved preset's name when one is being edited.
    original: Option<String>,
    /// An image chosen for a graphic watermark, copied when it is saved.
    source: Option<PathBuf>,
    /// The chooser's answer, from its own thread.
    picked: Arc<Mutex<Option<PathBuf>>>,
    /// The mark as last drawn for the preview, and what it was drawn from.
    preview: Option<(String, Option<(egui::TextureHandle, egui::Rect)>)>,
    /// The image or font the preview draws with, kept while only the
    /// settings change, and what it was loaded for.
    loaded: Option<(String, watermark::Ready)>,
    notice: Notice,
}
/// What the editor says beside the name field.
#[derive(Default)]
enum Notice {
    #[default]
    None,
    /// Installed fonts are still being listed; the preview waits for them.
    LoadingFonts,
    Error(String),
}
impl Notice {
    fn text(&self) -> &str {
        match self {
            Self::None => "",
            Self::LoadingFonts => "Loading fonts…",
            Self::Error(e) => e,
        }
    }
}
impl WatermarkEditor {
    pub(super) fn new(watermark: Watermark) -> Self {
        // Installed fonts are listed off the interface thread.
        std::thread::spawn(|| {
            let _ = watermark::fonts::families();
        });
        let original = (!watermark.name.is_empty()).then(|| watermark.name.clone());
        Self {
            watermark,
            original,
            source: None,
            picked: Default::default(),
            preview: None,
            loaded: None,
            notice: Notice::None,
        }
    }
}

impl Editor {
    pub(super) fn watermark_editor(&mut self, ctx: &egui::Context) {
        let Some(mut state) = self.exports.watermark_editor.take() else {
            return;
        };
        if let Some(path) = state.picked.lock().ok().and_then(|mut p| p.take()) {
            state.source = Some(path);
            state.watermark.style = Style::Graphic;
            state.preview = None;
        }
        let photo = self
            .preview
            .texture
            .as_ref()
            .map(|p| (p.id(), p.size_vec2()));
        let mut close = None;
        let mut delete = false;
        let response = egui::Modal::new(egui::Id::new("watermark-editor"))
            .backdrop_color(Color32::from_black_alpha(140))
            .frame(modal_frame())
            .show(ctx, |ui| {
                let (rect, _) = ui.allocate_exact_size(Vec2::new(WIDTH, HEIGHT), Sense::hover());
                ui.painter().text(
                    rect.left_top() + Vec2::new(24., 26.),
                    egui::Align2::LEFT_CENTER,
                    "Watermark Editor",
                    egui::FontId::proportional(16.),
                    theme::gray(236),
                );
                let preview = egui::Rect::from_min_max(
                    rect.left_top() + Vec2::new(24., 52.),
                    egui::pos2(rect.right() - PANEL - 40., rect.bottom() - 72.),
                );
                draw_preview(ui, &mut state, preview, photo);
                let panel = egui::Rect::from_min_max(
                    egui::pos2(rect.right() - PANEL - 24., rect.top() + 52.),
                    rect.right_bottom() - Vec2::new(24., 72.),
                );
                let mut content = ui.new_child(egui::UiBuilder::new().max_rect(panel));
                egui::ScrollArea::vertical()
                    .auto_shrink(false)
                    .show(&mut content, |ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(8., 6.);
                        controls(ui, &mut state, ctx);
                    });
                let footer = egui::Rect::from_min_max(
                    egui::pos2(rect.left() + 24., rect.bottom() - 56.),
                    egui::pos2(rect.right() - 24., rect.bottom() - 12.),
                );
                let mut bar = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(footer)
                        .layout(egui::Layout::right_to_left(egui::Align::Center)),
                );
                bar.spacing_mut().button_padding = Vec2::new(18., 6.);
                if primary_button(&mut bar, "Save").clicked() {
                    close = Some(true);
                }
                if bar
                    .add(egui::Button::new("Cancel").min_size(Vec2::new(84., 30.)))
                    .clicked()
                {
                    close = Some(false);
                }
                bar.add_space(12.);
                bar.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    if state.original.is_some()
                        && ui
                            .add(egui::Button::new("Delete").frame(false))
                            .on_hover_text("Delete this watermark and its image")
                            .clicked()
                    {
                        close = Some(false);
                        delete = true;
                    }
                    ui.label(egui::RichText::new("Name").color(theme::gray(150)));
                    ui.add(
                        egui::TextEdit::singleline(&mut state.watermark.name)
                            .desired_width(200.)
                            .hint_text("Watermark name"),
                    );
                    let notice = state.notice.text();
                    if !notice.is_empty() {
                        ui.label(
                            egui::RichText::new(notice)
                                .size(12.)
                                .color(Color32::from_rgb(230, 120, 100)),
                        );
                    }
                });
            });
        match close.or(response.should_close().then_some(false)) {
            Some(true) => {
                let graphic = state.watermark.style == Style::Graphic;
                let source = state.source.clone().filter(|_| graphic);
                if graphic && source.is_none() && state.watermark.image.is_none() {
                    state.notice = Notice::Error("Choose a PNG or JPEG image".into());
                    self.exports.watermark_editor = Some(state);
                    return;
                }
                let saved = watermark::save(
                    &state.watermark,
                    state.original.as_deref(),
                    source.as_deref(),
                );
                match saved {
                    Ok(saved) => {
                        // Export with Previous follows a renamed watermark.
                        if let Some(old) = &state.original
                            && *old != saved.name
                        {
                            self.update_previous(old, |previous| {
                                previous.watermark_name = saved.name.clone();
                            });
                        }
                        self.exports.draft.watermark = true;
                        self.exports.draft.watermark_name = saved.name;
                        self.exports.watermarks = watermark::presets();
                    }
                    Err(e) => {
                        state.notice = Notice::Error(format!("{e:#}"));
                        self.exports.watermark_editor = Some(state);
                    }
                }
            }
            Some(false) if delete => {
                let original = state.original.clone().unwrap_or_default();
                let saved = self
                    .exports
                    .watermarks
                    .iter()
                    .find(|w| w.name == original)
                    .cloned();
                if let Some(w) = saved
                    && let Err(e) = watermark::delete(&w)
                {
                    // Nothing else changes; the editor stays open to say so.
                    state.notice = Notice::Error(format!("Not deleted: {e:#}"));
                    self.exports.watermark_editor = Some(state);
                    return;
                }
                // A deleted watermark is never swapped for another.
                if self.exports.draft.watermark_name == original {
                    self.exports.draft.watermark = false;
                    self.exports.draft.watermark_name = watermark::SIMPLE_COPYRIGHT.into();
                }
                // Export with Previous neither uses it.
                self.update_previous(&original, |previous| {
                    previous.watermark = false;
                    previous.watermark_name = watermark::SIMPLE_COPYRIGHT.into();
                });
                self.exports.watermarks = watermark::presets();
            }
            Some(false) => {}
            None => self.exports.watermark_editor = Some(state),
        }
    }
    /// Changes the settings Export with Previous uses, when they use the
    /// watermark `name`.
    fn update_previous(
        &mut self,
        name: &str,
        change: impl FnOnce(&mut crate::export::ExportSettings),
    ) {
        if let Some(mut previous) = crate::export::ExportSettings::load()
            && previous.watermark_name == name
        {
            change(&mut previous);
            if let Err(e) = previous.save() {
                self.status = format!("Export settings not saved: {e:#}");
            }
        }
    }
}

/// The open photo with the mark where an export puts it.
fn draw_preview(
    ui: &mut egui::Ui,
    state: &mut WatermarkEditor,
    area: egui::Rect,
    photo: Option<(egui::TextureId, Vec2)>,
) {
    ui.painter().rect_filled(area, 2., theme::gray(24));
    let Some((texture, size)) = photo.filter(|(_, s)| s.x > 0. && s.y > 0.) else {
        ui.painter().text(
            area.center(),
            egui::Align2::CENTER_CENTER,
            "Open a photo to preview the watermark",
            egui::FontId::proportional(12.),
            theme::gray(130),
        );
        return;
    };
    let scale = (area.width() / size.x).min(area.height() / size.y);
    let shown = egui::Rect::from_center_size(area.center(), size * scale);
    ui.painter().image(
        texture,
        shown,
        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.)),
        Color32::WHITE,
    );
    // Drawn again only when the watermark or the preview's size changed.
    let key = format!(
        "{}|{:?}|{}x{}",
        serde_json::to_string(&state.watermark).unwrap_or_default(),
        state.source,
        shown.width().round(),
        shown.height().round()
    );
    if state.preview.as_ref().is_none_or(|(k, _)| *k != key) {
        let mark = mark_texture(ui.ctx(), state, shown);
        // Tried again next frame while fonts are still being listed.
        let loading = mark.is_none() && matches!(state.notice, Notice::LoadingFonts);
        state.preview = (!loading).then_some((key, mark));
    }
    if let Some((_, Some((texture, rect)))) = &state.preview {
        // Cut at the photo's edges, as the export is.
        ui.painter().with_clip_rect(shown).image(
            texture.id(),
            rect.translate(shown.min.to_vec2()),
            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.)),
            Color32::WHITE,
        );
    }
}

/// The mark for a photo shown at `shown`'s size, as a texture and where it
/// goes relative to the photo's corner.
fn mark_texture(
    ctx: &egui::Context,
    state: &mut WatermarkEditor,
    shown: egui::Rect,
) -> Option<(egui::TextureHandle, egui::Rect)> {
    let mut w = state.watermark.clone();
    let assets = format!(
        "{:?}|{:?}|{:?}|{}|{}",
        w.style, state.source, w.image, w.family, w.face
    );
    let ready = match &state.loaded {
        Some((key, ready)) if *key == assets => ready.with(&w),
        // An installed font is found once the fonts are listed, off this
        // thread; drawn after that.
        _ if w.style == Style::Text
            && w.family != watermark::fonts::INTER
            && watermark::fonts::families_if_listed().is_none() =>
        {
            state.notice = Notice::LoadingFonts;
            ui_repaint(ctx);
            return None;
        }
        _ => {
            // An image not saved yet is previewed from where it was chosen.
            let loaded = match (&state.source, w.style) {
                (Some(source), Style::Graphic) => {
                    let dir = source.parent()?.to_path_buf();
                    w.image = source.file_name().map(|n| n.to_string_lossy().into_owned());
                    w.ready_in(&dir)
                }
                _ => w.ready(),
            };
            match loaded {
                Ok(r) => {
                    state.loaded = Some((assets, r.clone()));
                    r
                }
                Err(e) => {
                    state.notice = Notice::Error(format!("{e:#}"));
                    return None;
                }
            }
        }
    };
    state.notice = Notice::None;
    let (pw, ph) = (shown.width().round() as u32, shown.height().round() as u32);
    let placed = ready.place(pw.max(1), ph.max(1))?;
    let pixels: Vec<Color32> = placed
        .rgba
        .iter()
        .map(|[r, g, b, a]| Color32::from_rgba_unmultiplied(byte(*r), byte(*g), byte(*b), byte(*a)))
        .collect();
    let image = egui::ColorImage {
        size: [placed.width, placed.height],
        source_size: Vec2::new(placed.width as f32, placed.height as f32),
        pixels,
    };
    let texture = ctx.load_texture("watermark-preview", image, egui::TextureOptions::LINEAR);
    let rect = egui::Rect::from_min_size(
        egui::pos2(placed.x as f32, placed.y as f32),
        Vec2::new(placed.width as f32, placed.height as f32),
    );
    Some((texture, rect))
}

fn heading(ui: &mut egui::Ui, title: &str) {
    ui.add_space(6.);
    ui.label(
        egui::RichText::new(title)
            .size(12.)
            .strong()
            .color(theme::gray(200)),
    );
}
fn slider(ui: &mut egui::Ui, label: &str, value: &mut f32, range: std::ops::RangeInclusive<f32>) {
    ui.horizontal(|ui| {
        ui.add_sized(
            Vec2::new(70., 20.),
            egui::Label::new(egui::RichText::new(label).size(12.).color(theme::gray(150))),
        );
        ui.spacing_mut().slider_width = 150.;
        ui.add(egui::Slider::new(value, range));
    });
}
/// `slider` for a fraction, shown in percent.
fn percent_slider(
    ui: &mut egui::Ui,
    label: &str,
    fraction: &mut f32,
    range: std::ops::RangeInclusive<f32>,
) {
    let mut percent = *fraction * 100.;
    slider(ui, label, &mut percent, range);
    *fraction = percent / 100.;
}
/// The label before a row's controls.
fn field_label(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).size(12.).color(theme::gray(150)));
}
/// A color channel from 0 to 1 as a byte.
fn byte(v: f32) -> u8 {
    (v.clamp(0., 1.) * 255. + 0.5) as u8
}

/// Style, image or text options, shadow and effects, as Lightroom's
/// editor has them.
fn controls(ui: &mut egui::Ui, state: &mut WatermarkEditor, ctx: &egui::Context) {
    let w = &mut state.watermark;
    heading(ui, "Watermark Style");
    ui.horizontal(|ui| {
        ui.selectable_value(&mut w.style, Style::Text, "Text");
        ui.selectable_value(&mut w.style, Style::Graphic, "Graphic");
    });
    match w.style {
        Style::Graphic => {
            heading(ui, "Image Options");
            ui.horizontal(|ui| {
                let name = state
                    .source
                    .as_ref()
                    .and_then(|p| p.file_name())
                    .map(|n| n.to_string_lossy().into_owned())
                    .or_else(|| w.image.clone())
                    .unwrap_or_else(|| "A PNG or JPEG image".into());
                ui.add(
                    egui::Label::new(egui::RichText::new(name).size(12.).color(theme::gray(180)))
                        .truncate(),
                );
                if ui.button("Choose…").clicked() {
                    let picked = state.picked.clone();
                    let ctx = ctx.clone();
                    std::thread::spawn(move || {
                        if let Some(file) = rfd::FileDialog::new()
                            .add_filter("PNG or JPEG", &["png", "jpg", "jpeg"])
                            .pick_file()
                            && let Ok(mut slot) = picked.lock()
                        {
                            *slot = Some(file);
                        }
                        ctx.request_repaint();
                    });
                }
            });
        }
        Style::Text => {
            heading(ui, "Text");
            ui.add(
                egui::TextEdit::multiline(&mut w.text)
                    .desired_rows(2)
                    .desired_width(f32::INFINITY),
            );
            heading(ui, "Text Options");
            // Installed fonts are still being listed at first: Inter until then.
            let listed = watermark::fonts::families_if_listed();
            if listed.is_none() {
                ui_repaint(ctx);
            }
            let inter = [watermark::fonts::inter()];
            let families: &[watermark::fonts::Family] = listed.unwrap_or(&inter);
            ui.horizontal(|ui| {
                field_label(ui, "Font");
                egui::ComboBox::from_id_salt("watermark-font")
                    .width(200.)
                    .selected_text(&w.family)
                    .show_ui(ui, |ui| {
                        for f in families {
                            if ui.selectable_label(w.family == f.name, &f.name).clicked() {
                                w.family = f.name.clone();
                                w.face =
                                    f.faces.first().map(|x| x.name.clone()).unwrap_or_default();
                            }
                        }
                    });
            });
            ui.horizontal(|ui| {
                field_label(ui, "Style");
                let faces = families
                    .iter()
                    .find(|f| f.name == w.family)
                    .map(|f| f.faces.clone())
                    .unwrap_or_default();
                egui::ComboBox::from_id_salt("watermark-face")
                    .width(200.)
                    .selected_text(&w.face)
                    .show_ui(ui, |ui| {
                        for face in &faces {
                            ui.selectable_value(&mut w.face, face.name.clone(), &face.name);
                        }
                    });
            });
            ui.horizontal(|ui| {
                field_label(ui, "Align");
                ui.selectable_value(&mut w.align, Align::Left, "Left");
                ui.selectable_value(&mut w.align, Align::Center, "Center");
                ui.selectable_value(&mut w.align, Align::Right, "Right");
                ui.add_space(12.);
                field_label(ui, "Color");
                // The picker in sRGB, as the color is stored and laid over
                // the photo.
                let mut srgb = w.color.map(byte);
                if ui.color_edit_button_srgb(&mut srgb).changed() {
                    w.color = srgb.map(|c| c as f32 / 255.);
                }
            });
            ui.checkbox(&mut w.shadow.enabled, "Shadow");
            ui.add_enabled_ui(w.shadow.enabled, |ui| {
                percent_slider(ui, "Opacity", &mut w.shadow.opacity, 0.0..=100.);
                percent_slider(ui, "Offset", &mut w.shadow.offset, 0.0..=40.);
                percent_slider(ui, "Radius", &mut w.shadow.radius, 0.0..=40.);
                slider(ui, "Angle", &mut w.shadow.angle, -180.0..=180.);
            });
        }
    }
    heading(ui, "Watermark Effects");
    percent_slider(ui, "Opacity", &mut w.opacity, 0.0..=100.);
    ui.horizontal(|ui| {
        field_label(ui, "Size");
        let proportional = matches!(w.size, Size::Proportional(_));
        if ui.selectable_label(proportional, "Proportional").clicked() && !proportional {
            w.size = Size::default();
        }
        ui.selectable_value(&mut w.size, Size::Fit, "Fit");
        ui.selectable_value(&mut w.size, Size::Fill, "Fill");
    });
    if let Size::Proportional(p) = &mut w.size {
        percent_slider(ui, "", p, 1.0..=100.);
    }
    percent_slider(ui, "Horizontal", &mut w.inset[0], 0.0..=50.);
    percent_slider(ui, "Vertical", &mut w.inset[1], 0.0..=50.);
    ui.horizontal(|ui| {
        field_label(ui, "Anchor");
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing = Vec2::splat(2.);
            for v in 0..3 {
                ui.horizontal(|ui| {
                    for h in 0..3 {
                        ui.radio_value(&mut w.anchor, Anchor(h, v), "");
                    }
                });
            }
        });
        ui.add_space(16.);
        field_label(ui, "Rotate");
        if ui
            .add(egui::Button::new("⟲").frame(false))
            .on_hover_text("Rotate left")
            .clicked()
        {
            w.rotation = (w.rotation + 1) % 4;
        }
        if ui
            .add(egui::Button::new("⟳").frame(false))
            .on_hover_text("Rotate right")
            .clicked()
        {
            w.rotation = (w.rotation + 3) % 4;
        }
    });
}

/// Asks for another frame soon, while something loads elsewhere.
fn ui_repaint(ctx: &egui::Context) {
    ctx.request_repaint_after(std::time::Duration::from_millis(250));
}
