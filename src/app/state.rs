//! State owned by the document, preview, viewport and preset browser.
use crate::{
    develop::Recipe,
    export::ExportOptions,
    raw::{CameraImage, Metadata},
};
use eframe::egui::{self, Vec2};
use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Instant};

#[derive(Default)]
pub(super) struct Document {
    pub(super) save: super::save_state::SaveState,
    pub(super) history: super::history::History,
    pub(super) path: Option<PathBuf>,
    pub(super) files: Vec<PathBuf>,
    pub(super) metadata: Option<Metadata>,
    images: Option<DecodedImages>,
    pub(super) recipe: Recipe,
    pub(super) export: ExportOptions,
    pub(super) catalog_photo: Option<i64>,
    pub(super) lightroom_notice: String,
    /// Lightroom's history for the open catalog photo, oldest first.
    pub(super) lightroom_history: Vec<crate::catalog::HistoryStep>,
    /// Apply the photo's Lightroom settings once its profiles arrive.
    pub(super) pending_lightroom: bool,
    pub(super) profiles: Vec<Arc<crate::camera_profiles::CameraProfile>>,
    pub(super) profile_errors: Vec<String>,
}

/// What the preview texture holds: the whole photo, or a 1:1 region of it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) enum TextureMode {
    #[default]
    Whole,
    Region([u32; 4]),
}
pub(super) struct PreviewState {
    pub(super) task: super::task::Task,
    pub(super) texture: Option<egui::TextureHandle>,
    /// Small copy of the last whole-photo render for the Navigator.
    pub(super) navigator: Option<egui::TextureHandle>,
    pub(super) thumbs: HashMap<PathBuf, egui::TextureHandle>,
    pub(super) histogram: [[u32; 256]; 3],
    pub(super) status: String,
    pub(super) last_fit_edge: u32,
    pub(super) last_region: Option<[u32; 4]>,
    pub(super) mode: TextureMode,
    pub(super) pending_mode: TextureMode,
}
impl Default for PreviewState {
    fn default() -> Self {
        Self {
            task: Default::default(),
            texture: None,
            navigator: None,
            thumbs: HashMap::new(),
            histogram: [[0; 256]; 3],
            status: String::new(),
            last_fit_edge: 0,
            last_region: None,
            mode: TextureMode::Whole,
            pending_mode: TextureMode::Whole,
        }
    }
}

pub(super) struct ViewState {
    pub(super) zoom100: bool,
    pub(super) pan: [f32; 2],
    pub(super) viewport: Vec2,
    pub(super) compare: bool,
    pub(super) clipping: bool,
    pub(super) crop_mode: bool,
    pub(super) crop_drag: Option<([f32; 4], usize)>,
    pub(super) aspect: f32,
    pub(super) picker: bool,
    pub(super) monitor: Option<PathBuf>,
    pub(super) selected_band: usize,
    pub(super) selected_grade: usize,
    pub(super) selected_curve: usize,
    pub(super) parametric_curve: bool,
    pub(super) mixer_color: bool,
    pub(super) mixer_adjust: usize,
    pub(super) shortcuts: bool,
    /// Zoom when not in Fit: screen pixels per image pixel (1 = 100%).
    pub(super) zoom_level: f32,
    pub(super) zoom_key: (bool, f32),
    pub(super) zoom_anim: Option<(f64, egui::Rect)>,
    pub(super) shown_rect: Option<egui::Rect>,
}
impl Default for ViewState {
    fn default() -> Self {
        Self {
            zoom100: false,
            pan: [0.5, 0.5],
            viewport: Vec2::ZERO,
            compare: false,
            clipping: false,
            crop_mode: false,
            crop_drag: None,
            aspect: -1.,
            picker: false,
            monitor: None,
            selected_band: 0,
            selected_grade: 1,
            selected_curve: 0,
            parametric_curve: false,
            mixer_color: false,
            mixer_adjust: 0,
            shortcuts: false,
            zoom_level: 1.,
            zoom_key: (false, 1.),
            zoom_anim: None,
            shown_rect: None,
        }
    }
}

#[derive(Default)]
pub(super) struct PresetBrowser {
    pub(super) library: Arc<crate::presets::Library>,
    pub(super) issues: Vec<Option<String>>,
    pub(super) filter: String,
    pub(super) favorites: std::collections::BTreeSet<String>,
    pub(super) compatible_only: bool,
    pub(super) favorites_only: bool,
    pub(super) selected: String,
    pub(super) preview: Option<Recipe>,
    pub(super) hover: Option<(usize, Instant)>,
}

impl Document {
    pub fn reset(&mut self, catalog_photo: Option<i64>) {
        *self = Self {
            catalog_photo,
            ..Default::default()
        };
    }
}
impl PreviewState {
    pub fn clear_document(&mut self) {
        self.task.invalidate();
        self.texture = None;
        self.navigator = None;
        self.thumbs.clear();
        self.histogram = [[0; 256]; 3];
        self.status.clear();
        self.last_fit_edge = 0;
        self.last_region = None;
        self.mode = TextureMode::Whole;
    }
}
impl ViewState {
    pub fn clear_document(&mut self) {
        self.zoom100 = false;
        self.zoom_anim = None;
        self.shown_rect = None;
        self.crop_mode = false;
        self.crop_drag = None;
        self.picker = false;
        self.compare = false;
    }
}
impl PresetBrowser {
    pub fn clear_document(&mut self) {
        self.issues.clear();
        self.selected.clear();
        self.preview = None;
        self.hover = None;
    }
}

struct DecodedImages {
    full: Arc<CameraImage>,
    draft: Arc<CameraImage>,
}
impl Document {
    pub fn full(&self) -> Option<&Arc<CameraImage>> {
        self.images.as_ref().map(|images| &images.full)
    }
    pub fn draft(&self) -> Option<&Arc<CameraImage>> {
        self.images.as_ref().map(|images| &images.draft)
    }
    pub fn set_images(&mut self, full: Arc<CameraImage>, draft: Arc<CameraImage>) {
        self.images = Some(DecodedImages { full, draft });
    }
}
