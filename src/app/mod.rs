//! Desktop composition and presentation, built on the crate's domain APIs.
//!
//! `Editor` coordinates the workspace; `state` separates the document, preview,
//! viewport and preset browser. `workflow` starts operations, `events` accepts
//! worker results, and `workspace` composes each frame. Task cancellation, history,
//! foreground activity and save policy have their own modules.
//!
//! File formats, persistence and pixel processing belong in the domain modules.
//! See `docs/code-map.md` for panel, library and worker implementation locations.
use crate::{
    app::worker::{Event, Latest, LoadJob, RenderJob},
    develop::Recipe,
};
use eframe::egui::{self, Color32, Stroke, Vec2};
use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
};

pub struct Editor {
    activity: activity::Activity,
    load: task::Task,
    presets: PresetBrowser,
    view: ViewState,
    preview: PreviewState,
    document: Document,
    context: egui::Context,
    session_file: Option<PathBuf>,
    library: Option<Box<crate::app::library::Library>>,
    library_mode: bool,
    tx: Sender<Event>,
    rx: Receiver<Event>,
    loader: Latest<LoadJob>,
    renderer: Latest<RenderJob>,
    clipboard: Option<Recipe>,
    /// Collapsed panel sections as last saved to the session.
    collapsed: std::collections::BTreeSet<String>,
    onboarding: onboarding::Onboarding,
    onboarding_done: bool,
    preferences: preferences::Preferences,
    exports: export::Exports,
    /// Library/Develop position to restore once the session's catalog opens.
    restore: Option<(String, Option<i64>, bool)>,
    /// That position as last written to the session.
    saved_place: (String, Option<i64>, bool),
    status: String,
    close_confirm: bool,
}
impl Editor {
    pub fn new(cc: &eframe::CreationContext<'_>, path: Option<PathBuf>) -> Self {
        install_fallback_fonts(&cc.egui_ctx);
        Self::with_backend(
            &cc.egui_ctx,
            path,
            crate::storage::load_session(),
            Some(crate::storage::data_dir().join("session.json")),
            worker::RenderBackend::Gpu,
        )
    }
    #[cfg(test)]
    fn with_context(
        ctx: &egui::Context,
        path: Option<PathBuf>,
        session: crate::storage::Session,
        session_file: Option<PathBuf>,
    ) -> Self {
        Self::with_backend(ctx, path, session, session_file, worker::RenderBackend::Cpu)
    }
    fn with_backend(
        ctx: &egui::Context,
        path: Option<PathBuf>,
        session: crate::storage::Session,
        session_file: Option<PathBuf>,
        backend: worker::RenderBackend,
    ) -> Self {
        crate::raw::set_demosaic(session.demosaic);
        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = Color32::from_gray(35);
        visuals.window_fill = Color32::from_gray(35);
        visuals.extreme_bg_color = Color32::from_gray(22);
        visuals.faint_bg_color = Color32::from_gray(40);
        visuals.selection.bg_fill = Color32::from_rgb(62, 88, 115);
        visuals.widgets.inactive.bg_fill = Color32::from_gray(43);
        visuals.widgets.inactive.weak_bg_fill = Color32::from_gray(43);
        visuals.widgets.noninteractive.fg_stroke = Stroke::new(1., Color32::from_gray(194));
        visuals.widgets.hovered.bg_fill = Color32::from_gray(59);
        visuals.widgets.hovered.weak_bg_fill = Color32::from_gray(59);
        visuals.widgets.active.bg_fill = Color32::from_gray(67);
        visuals.widgets.active.weak_bg_fill = Color32::from_gray(67);
        // egui insets button text by the stroke width, but an unframed item
        // (a menu or list row) has no stroke until hovered, so its text moved
        // by a pixel. No widget strokes: fills alone show state.
        for widget in [
            &mut visuals.widgets.inactive,
            &mut visuals.widgets.hovered,
            &mut visuals.widgets.active,
            &mut visuals.widgets.open,
        ] {
            widget.bg_stroke = Stroke::NONE;
        }
        // egui grows hovered widgets by a pixel; keep every control a fixed size.
        for widget in [
            &mut visuals.widgets.hovered,
            &mut visuals.widgets.active,
            &mut visuals.widgets.open,
        ] {
            widget.expansion = 0.;
        }
        ctx.set_visuals(visuals);
        // Cmd/Ctrl + and − zoom the photo, not the whole interface.
        ctx.options_mut(|o| o.zoom_with_keyboard = false);
        ctx.data_mut(|d| {
            d.insert_temp(widgets::collapsed_sections_id(), session.collapsed.clone())
        });
        ctx.style_mut_of(egui::Theme::Dark, |style| {
            style.spacing.item_spacing = Vec2::new(8., 5.);
            style.spacing.button_padding = Vec2::new(9., 5.);
            style.spacing.indent = 18.;
            style
                .text_styles
                .insert(egui::TextStyle::Body, egui::FontId::proportional(13.));
            style
                .text_styles
                .insert(egui::TextStyle::Button, egui::FontId::proportional(13.));
            style
                .text_styles
                .insert(egui::TextStyle::Small, egui::FontId::proportional(11.));
        });
        // Only a real session (not an isolated test) shows first-run setup.
        let show_onboarding = !session.onboarding_done && session_file.is_some();
        let path = path.or(session.last_path.filter(|p| p.exists()));
        let (tx, rx) = mpsc::channel();
        let loader = worker::loader(tx.clone(), ctx.clone());
        let renderer = worker::renderer_with_backend(tx.clone(), ctx.clone(), backend);
        let mut app = Self {
            activity: Default::default(),
            load: Default::default(),
            presets: PresetBrowser {
                favorites: crate::presets::load_favorites(),
                ..Default::default()
            },
            view: ViewState {
                monitor: session.monitor,
                ..Default::default()
            },
            preview: Default::default(),
            document: Default::default(),
            context: ctx.clone(),
            session_file,
            library: None,
            library_mode: false,
            tx,
            rx,
            loader,
            renderer,
            clipboard: None,
            collapsed: session.collapsed.clone(),
            onboarding: onboarding::Onboarding::new(show_onboarding),
            onboarding_done: session.onboarding_done,
            preferences: Default::default(),
            exports: Default::default(),
            restore: Some((
                session.library_source.clone(),
                session.selected_photo,
                session.develop,
            )),
            saved_place: (
                session.library_source.clone(),
                session.selected_photo,
                session.develop,
            ),
            status: "Open a RAW photo to begin".into(),
            close_confirm: false,
        };
        app.reload_presets(ctx);
        if let Some(path) = path {
            app.open(path);
        }
        app
    }
    // A caller may disable preference persistence, e.g. in an isolated UI test.
    fn save_session(&self) -> anyhow::Result<()> {
        if let Some(path) = &self.session_file {
            crate::storage::atomic_json(
                path,
                &crate::storage::Session {
                    last_path: self.session_path(),
                    monitor: self.view.monitor.clone(),
                    collapsed: self.collapsed.clone(),
                    onboarding_done: self.onboarding_done,
                    library_source: self.saved_place.0.clone(),
                    selected_photo: self.saved_place.1,
                    develop: self.saved_place.2,
                    demosaic: crate::raw::demosaic(),
                },
            )?;
        }
        Ok(())
    }
    /// The Library folder, selected photo and module, as saved in the session.
    fn current_place(&self) -> (String, Option<i64>, bool) {
        let Some(library) = &self.library else {
            return Default::default();
        };
        let develop = !self.library_mode && self.document.catalog_photo.is_some();
        let photo = if develop {
            self.document.catalog_photo
        } else {
            library.selected
        };
        (library.source_key().to_string(), photo, develop)
    }
    fn session_path(&self) -> Option<PathBuf> {
        self.library
            .as_ref()
            .map(|l| l.catalog.path.clone())
            .or_else(|| self.document.path.clone())
    }
}
impl eframe::App for Editor {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.draw(ui);
    }
}
pub fn run(path: Option<PathBuf>) -> anyhow::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("RAWmakase")
            .with_inner_size([1440., 960.])
            .with_min_inner_size([900., 650.]),
        ..Default::default()
    };
    eframe::run_native(
        "RAWmakase",
        options,
        Box::new(move |cc| Ok(Box::new(Editor::new(cc, path)))),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))
}

mod catalog;
mod dialogs;
mod export;
mod inspector;
pub mod library;
mod onboarding;
mod photo_metadata;
mod preferences;
mod presets;
#[cfg(test)]
mod tests;
mod viewport;
mod widgets;
pub mod worker;
mod workflow;
mod workspace;

mod state;
use state::{Document, PresetBrowser, PreviewState, ViewState};

mod history;

mod task;

mod activity;

mod save_state;

mod editing;
mod toolbar;

mod events;

/// egui's bundled fonts miss many symbols that preset and profile names use
/// (superscripts, arrows, ◊…). Add a broad-coverage system font as the last
/// fallback so they render instead of showing boxes. Missing files are skipped.
fn install_fallback_fonts(ctx: &egui::Context) {
    const CANDIDATES: [&str; 6] = [
        "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
        "/System/Library/Fonts/Apple Symbols.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/TTF/DejaVuSans.ttf",
        "/usr/share/fonts/noto/NotoSans-Regular.ttf",
        "C:\\Windows\\Fonts\\seguisym.ttf",
    ];
    let mut fonts = egui::FontDefinitions::default();
    let mut added = false;
    for (i, path) in CANDIDATES.iter().enumerate() {
        if let Ok(bytes) = std::fs::read(path) {
            let name = format!("fallback-{i}");
            fonts.font_data.insert(
                name.clone(),
                std::sync::Arc::new(egui::FontData::from_owned(bytes)),
            );
            for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                fonts.families.entry(family).or_default().push(name.clone());
            }
            added = true;
        }
    }
    if added {
        ctx.set_fonts(fonts);
    }
}
