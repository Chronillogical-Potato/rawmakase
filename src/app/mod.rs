//! Desktop composition and presentation, built on the crate's domain APIs.
//!
//! `Editor` coordinates the workspace; `state` separates the document, preview,
//! viewport and preset browser. `workflow` starts operations, `events` accepts
//! worker results, and `workspace` composes each frame. Task cancellation, history,
//! foreground activity and save policy have their own modules.
//!
//! File formats, persistence and pixel processing belong in the domain modules.
//! See `docs/code-map.md` for panel, library and worker implementation locations.
use crate::app::worker::{Event, Latest, LoadJob};
#[cfg(test)]
use crate::develop::Recipe;
use eframe::egui::{self, Vec2};
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
    renderer: worker::Renderer,
    /// Develop's Reference View, and the worker developing its photo.
    reference: reference::ReferenceView,
    reference_loader: Latest<worker::ReferenceJob>,
    clipboard: Option<settings_transfer::Clipboard>,
    /// The settings of the photo open before this one, for Paste from Previous.
    previous_settings: Option<settings_transfer::Settings>,
    /// Copy Settings while open, and the groups it last copied.
    copy_dialog: Option<settings_transfer::CopyDialog>,
    copy_groups: crate::develop::settings_groups::GroupSelection,
    /// A preset made here being renamed.
    preset_rename: Option<user_presets::PresetRename>,
    /// Collapsed panel sections as last saved to the session.
    collapsed: std::collections::BTreeSet<String>,
    /// The sides in Solo Mode as last saved to the session.
    solo: std::collections::BTreeSet<String>,
    onboarding: onboarding::Onboarding,
    onboarding_done: bool,
    preferences: preferences::Preferences,
    updates: updates::Updates,
    #[cfg(feature = "telemetry")]
    stats: stats::UsageStats,
    themes: theme::Themes,
    exports: export::Exports,
    autosave: autosave::Autosave,
    /// Library/Develop position to restore once the session's catalog opens.
    restore: Option<(String, Option<i64>, bool)>,
    /// A photo from outside the Library to open once the catalog is ready,
    /// and whether its folder has already been added.
    pending_photo: Option<(PathBuf, bool)>,
    /// Cancels the prefetch started for the photo on screen.
    prefetch_cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// That position as last written to the session.
    saved_place: (String, Option<i64>, bool),
    /// How the Library showed its photos, as last written to the session;
    /// returned to whenever the catalog is loaded.
    saved_layout: crate::storage::LibraryLayout,
    status: String,
    /// What a running catalog import or open is doing.
    catalog_work: Option<String>,
    /// Progress of a running profile or preset import.
    importing: Option<std::sync::Arc<std::sync::Mutex<String>>>,
    close_confirm: bool,
    /// The virtual copy waiting for the user to confirm its removal.
    remove_copy: Option<i64>,
    /// Photos waiting for the user to confirm Read Metadata from Files.
    read_metadata: Option<Vec<i64>>,
    /// A photo Develop could not open and why, until the user dismisses it.
    not_editable: Option<(String, String)>,
    /// Cmd+Z across Library and Develop.
    undo_log: undo::UndoLog,
    /// Photo > Auto Advance, saved in the session.
    auto_advance: bool,
    /// Preferences: whether converting to black & white applies the Auto mix to a
    /// mix never set, as Lightroom's preference of that name (on by default).
    first_conversion: treatment::FirstConversion,
    /// The RAW the Loupe last started loading, so one that fails is not
    /// loaded again every frame.
    loupe_tried: Option<i64>,
    /// Preferences > Raw Defaults, ready to apply; shared with the loader and the
    /// Library's previews.
    raw_defaults: std::sync::Arc<crate::develop::defaults::DevelopDefaults>,
}
impl Editor {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        path: Option<PathBuf>,
        launch: crate::updates::Launch,
    ) -> Self {
        // The desktop's hinting and antialiasing, read once (a D-Bus call on
        // Linux) and applied to the fonts here and to the visuals below.
        let text = fastframe_text::detect();
        if let Some(render_state) = &cc.wgpu_render_state {
            survive_surface_errors(&render_state.device);
        }
        install_fonts(&cc.egui_ctx, &text);
        icons::install(&cc.egui_ctx);
        let mut editor = Self::with_backend(
            &cc.egui_ctx,
            path,
            crate::storage::load_session(),
            Some(crate::storage::data_dir().join("session.json")),
            worker::RenderBackend::Gpu(cc.wgpu_render_state.clone()),
        );
        editor.updates.launched(launch, &mut editor.status);
        cc.egui_ctx
            .all_styles_mut(|style| text.apply_to_visuals(&mut style.visuals));
        editor.themes.text = text;
        editor.themes.start();
        editor
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
        theme::apply(
            ctx,
            theme::Palette::DEFAULT,
            &fastframe_text::TextRendering::platform_default(),
        );
        // Cmd/Ctrl + and − zoom the photo, not the whole interface.
        ctx.options_mut(|o| o.zoom_with_keyboard = false);
        ctx.data_mut(|d| {
            d.insert_temp(widgets::collapsed_sections_id(), session.collapsed.clone());
            d.insert_temp(widgets::solo_sections_id(), session.solo.clone());
        });
        ctx.all_styles_mut(|style| {
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
        // Only a real session checks GitHub, not an isolated test.
        let updates = updates::Updates::new(&session, session_file.is_some().then_some(ctx));
        #[cfg(feature = "telemetry")]
        let adapter = match &backend {
            worker::RenderBackend::Gpu(Some(render_state)) => Some(render_state.adapter.get_info()),
            _ => None,
        };
        #[cfg(feature = "telemetry")]
        let stats = stats::UsageStats::new(
            adapter.as_ref(),
            session_file
                .as_deref()
                .and_then(std::path::Path::parent)
                .map(|dir| (dir.to_path_buf(), ctx)),
        );
        let last = session.last_path.clone().filter(|p| p.exists());
        let (tx, rx) = mpsc::channel();
        let loader = worker::loader(tx.clone(), ctx.clone());
        let renderer = worker::renderer_with_backend(tx.clone(), ctx.clone(), backend);
        let reference_loader = worker::reference_loader(tx.clone(), ctx.clone());
        let mut app = Self {
            activity: Default::default(),
            load: Default::default(),
            presets: PresetBrowser {
                favorites: crate::presets::load_favorites(),
                ..Default::default()
            },
            view: ViewState {
                monitor: session.monitor,
                crop_guides: crop_tool::CropGuides::from_session(&session.crop_guides),
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
            reference: Default::default(),
            reference_loader,
            clipboard: None,
            previous_settings: None,
            copy_dialog: None,
            preset_rename: None,
            copy_groups: session.copy_groups.clone().unwrap_or_default(),
            collapsed: session.collapsed.clone(),
            solo: session.solo.clone(),
            onboarding: onboarding::Onboarding::new(show_onboarding),
            onboarding_done: session.onboarding_done,
            preferences: Default::default(),
            updates,
            #[cfg(feature = "telemetry")]
            stats,
            themes: theme::Themes::new(
                ctx,
                // Sessions from before `theme_chosen` saved only a palette.
                (session.theme_chosen || session.theme.is_some()).then(|| session.theme.clone()),
                fastframe_text::TextRendering::platform_default(),
            ),
            exports: Default::default(),
            autosave: Default::default(),
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
            saved_layout: session.library_layout.clone(),
            pending_photo: None,
            prefetch_cancel: Default::default(),
            status: "Pick a photo in the Library to begin".into(),
            catalog_work: None,
            importing: None,
            close_confirm: false,
            remove_copy: None,
            read_metadata: None,
            not_editable: None,
            undo_log: Default::default(),
            auto_advance: session.auto_advance,
            first_conversion: if session.no_auto_black_white_mix {
                treatment::FirstConversion::KeepMix
            } else {
                treatment::FirstConversion::AutoMix
            },
            loupe_tried: None,
            raw_defaults: std::sync::Arc::new(crate::develop::defaults::DevelopDefaults::load(
                session.raw_defaults.clone(),
            )),
        };
        app.reload_presets(ctx);
        // A catalog passed on the command line opens instead of the last one;
        // a photo passed there is added to the last catalog.
        match path {
            Some(path) if path.extension().is_some_and(|e| e == "rawmakase") => app.open(path),
            path => {
                if let Some(last) = last {
                    app.open(last);
                }
                if let Some(path) = path {
                    app.open(path);
                }
            }
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
                    solo: self.solo.clone(),
                    onboarding_done: self.onboarding_done,
                    library_source: self.saved_place.0.clone(),
                    selected_photo: self.saved_place.1,
                    develop: self.saved_place.2,
                    demosaic: crate::raw::demosaic(),
                    no_update_checks: !self.updates.automatic,
                    skipped_version: self.updates.skipped.clone(),
                    theme: self.themes.chosen().flatten(),
                    theme_chosen: self.themes.chosen().is_some(),
                    auto_advance: self.auto_advance,
                    no_auto_black_white_mix: self.first_conversion
                        == treatment::FirstConversion::KeepMix,
                    library_layout: self.saved_layout.clone(),
                    copy_groups: Some(self.copy_groups.clone()),
                    crop_guides: self.view.crop_guides.to_session(),
                    raw_defaults: self.raw_defaults.settings().clone(),
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
            library.selected()
        };
        (library.source_key(), photo, develop)
    }
    fn session_path(&self) -> Option<PathBuf> {
        self.library
            .as_ref()
            .map(|l| l.catalog.path.clone())
            .or_else(|| self.document.path.clone())
    }
}
impl eframe::App for Editor {
    /// What shows where no panel paints, e.g. behind the Library grid: the
    /// theme's darkest grey rather than eframe's near-black.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        theme::gray(12).to_normalized_gamma_f32()
    }
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        // AppKit moves the traffic lights back during layout passes.
        fastframe_macos::align_traffic_lights(frame, ui.ctx(), workspace::BAR_HEIGHT);
        self.draw(ui);
    }
}
pub fn run(path: Option<PathBuf>, launch: crate::updates::Launch) -> anyhow::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("RAWmakase")
            // An empty icon keeps the bundle's and desktop entry's icon;
            // otherwise eframe replaces it with egui's logo while running.
            .with_icon(egui::IconData::default())
            .with_inner_size([1440., 960.])
            .with_min_inner_size([900., 650.])
            // On macOS the workspace bar is the title bar, under the traffic
            // lights; elsewhere the window keeps its decorations.
            .with_fullsize_content_view(true)
            .with_titlebar_shown(false)
            .with_title_shown(false),
        // fastframe-macos turns on eframe's glow renderer too; previews
        // share the UI's wgpu device.
        renderer: eframe::Renderer::Wgpu,
        wgpu_options: wgpu_options(),
        ..Default::default()
    };
    eframe::run_native(
        "RAWmakase",
        options,
        Box::new(move |cc| Ok(Box::new(Editor::new(cc, path, launch)))),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))
}
/// The UI's wgpu device also renders previews (see `develop::gpu`): prefer the
/// discrete GPU and ask for the storage limits full-resolution regions need.
fn wgpu_options() -> eframe::egui_wgpu::WgpuConfiguration {
    let mut options = eframe::egui_wgpu::WgpuConfiguration::default();
    if let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = &mut options.wgpu_setup {
        setup.power_preference = wgpu::PowerPreference::HighPerformance;
        let default = setup.device_descriptor.clone();
        setup.device_descriptor = std::sync::Arc::new(move |adapter| {
            let mut descriptor = default(adapter);
            if adapter.get_info().backend != wgpu::Backend::Gl {
                descriptor.required_limits = crate::develop::gpu::required_limits(adapter);
            }
            descriptor
        });
    }
    // A surface whose configuration failed reports `Validation` until it is
    // configured again; the default would skip frames until the next resize.
    let default = options.on_surface_status.clone();
    options.on_surface_status = std::sync::Arc::new(move |status| match status {
        wgpu::CurrentSurfaceTexture::Validation => {
            eframe::egui_wgpu::SurfaceErrorAction::Reconfigure
        }
        _ => default(status),
    });
    options
}

/// wgpu panics on uncaptured errors by default. Reconfiguring the window's
/// surface can fail transiently, for example when a tiling compositor resizes
/// the window while the GPU is busy ("Failed to wait for GPU to come idle"),
/// and the next frame configures it again. Other errors are still bugs.
fn survive_surface_errors(device: &wgpu::Device) {
    device.on_uncaptured_error(std::sync::Arc::new(|error| {
        if error.to_string().contains("In Surface::") {
            eprintln!("Ignoring window surface error: {error}");
        } else {
            panic!("wgpu error: {error}");
        }
    }));
}

mod auto;
mod before_after;
mod brush_scroll;
mod bulk_import;
mod catalog;
mod clipping;
mod color_grading;
mod crop_tool;
mod dialogs;
mod export;
mod guided_tool;
mod inspector;
pub mod library;
mod mask_tool;
mod navigator;
mod onboarding;
mod overlay;
mod photo_metadata;
mod point_color_panel;
mod preferences;
mod presets;
mod raw_defaults;
mod red_eye_tool;
mod reference;
mod retouch_tool;
#[cfg(feature = "telemetry")]
mod stats;
mod stroke_outline;
#[cfg(test)]
mod tests;
mod undo;
mod upright;
mod viewport;
mod widgets;
pub mod worker;
mod workflow;
mod workspace;

mod state;
use state::{Document, PresetBrowser, PreviewState, ViewState};

mod history;

mod icons;

mod task;

mod activity;

mod autosave;
mod save_state;

mod editing;
mod settings_transfer;
mod shortcuts;
mod snapshots;
mod sync;
mod theme;
mod tone_drag;
mod toolbar;
mod treatment;
mod updates;
mod user_presets;

mod events;

/// Inter, with tabular figures so values keep their width as they change,
/// and an installed face for every script and symbol it lacks (fastframe-fonts),
/// rendered as the desktop renders text.
fn install_fonts(ctx: &egui::Context, text: &fastframe_text::TextRendering) {
    let mut fonts = fastframe_fonts::FontSetup::default().definitions();
    text.apply_to(&mut fonts);
    ctx.set_fonts(fonts);
}
