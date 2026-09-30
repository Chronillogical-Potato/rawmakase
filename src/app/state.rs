//! State owned by the document, preview, viewport and preset browser.
use crate::{
    develop::Recipe,
    export::ExportOptions,
    raw::{CameraImage, Metadata},
};
use eframe::egui::{self, Vec2};
use std::{path::PathBuf, sync::Arc, time::Instant};

#[derive(Default)]
pub(super) struct Document {
    pub(super) save: super::save_state::SaveState,
    pub(super) history: super::history::History,
    pub(super) path: Option<PathBuf>,
    pub(super) metadata: Option<Metadata>,
    image: Option<Arc<CameraImage>>,
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
    /// The Auto estimate for this photo; dropping it with the document cancels it.
    pub(super) auto: super::task::Task,
    /// What the running estimate measures: the recipe without the settings Auto sets.
    pub(super) auto_input: Option<Recipe>,
    /// The recipe as Auto last left it; while it is unchanged, Auto has nothing to do.
    pub(super) auto_applied: Option<Recipe>,
    /// The Transform panel's Upright analysis for this photo.
    pub(super) upright: super::task::Task,
}

/// What the latest render showed: the whole photo, or a 1:1 region of it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) enum TextureMode {
    #[default]
    Whole,
    Region([u32; 4]),
}
/// A texture the viewport draws: uploaded through egui, or presented by the GPU
/// renderer into a texture it registered.
#[derive(Clone)]
pub(super) struct Picture {
    id: egui::TextureId,
    size: [usize; 2],
    /// Keeps an uploaded texture alive; presented ones belong to the renderer.
    handle: Option<egui::TextureHandle>,
}
impl Picture {
    pub(super) fn presented(id: egui::TextureId, size: [usize; 2]) -> Self {
        Self {
            id,
            size,
            handle: None,
        }
    }
    pub(super) fn id(&self) -> egui::TextureId {
        self.id
    }
    /// Presented into a texture the renderer registered, rather than uploaded.
    fn is_presented(&self) -> bool {
        self.handle.is_none()
    }
    pub(super) fn size_vec2(&self) -> Vec2 {
        Vec2::new(self.size[0] as f32, self.size[1] as f32)
    }
    /// Shows `image` in `slot`, reusing its uploaded texture when it has one.
    pub(super) fn upload(
        slot: &mut Option<Picture>,
        ctx: &egui::Context,
        name: &str,
        image: egui::ColorImage,
    ) {
        let size = image.size;
        match slot.as_mut().and_then(|p| p.handle.clone()) {
            Some(mut handle) => {
                handle.set(image, egui::TextureOptions::LINEAR);
                *slot = Some(handle.into());
            }
            None => {
                *slot = Some(
                    ctx.load_texture(name, image, egui::TextureOptions::LINEAR)
                        .into(),
                )
            }
        }
        debug_assert_eq!(slot.as_ref().map(|p| p.size), Some(size));
    }
}
impl From<egui::TextureHandle> for Picture {
    fn from(handle: egui::TextureHandle) -> Self {
        Self {
            id: handle.id(),
            size: handle.size(),
            handle: Some(handle),
        }
    }
}
pub(super) struct PreviewState {
    pub(super) task: super::task::Task,
    /// The last whole-photo render, always drawn so zooming never shows a gap.
    pub(super) texture: Option<Picture>,
    /// The last 100% region render, drawn over `texture` while `mode` is a region.
    pub(super) region: Option<Picture>,
    /// Small copy of the last whole-photo render for the Navigator.
    pub(super) navigator: Option<Picture>,
    pub(super) histogram: [[u32; 256]; 3],
    /// The shown pixels of `texture` and `region` while the white balance selector
    /// is active, for its loupe.
    pub(super) samples: Option<image::RgbImage>,
    pub(super) region_samples: Option<image::RgbImage>,
    /// Whether a render with loupe samples was asked for since the selector opened.
    pub(super) samples_requested: bool,
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
            region: None,
            navigator: None,
            histogram: [[0; 256]; 3],
            samples: None,
            region_samples: None,
            samples_requested: false,
            status: String::new(),
            last_fit_edge: 0,
            last_region: None,
            mode: TextureMode::Whole,
            pending_mode: TextureMode::Whole,
        }
    }
}

/// The tool that owns clicks and drags on the photo, as in Lightroom's tool strip.
/// Only one is active; activating one closes the others.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Tool {
    #[default]
    None,
    Crop,
    WhiteBalance,
    /// Spot removal: Heal and Clone.
    Remove,
    Mask,
}
pub(super) struct ViewState {
    pub(super) zoom100: bool,
    pub(super) pan: [f32; 2],
    pub(super) viewport: Vec2,
    pub(super) compare: bool,
    pub(super) clipping: bool,
    pub(super) tool: Tool,
    pub(super) crop_drag: Option<([f32; 4], usize)>,
    pub(super) aspect: f32,
    /// Spot removal settings, selection and drag in progress.
    pub(super) retouch: super::retouch_tool::RetouchTool,
    /// Masking panel state.
    pub(super) masking: super::mask_tool::MaskTool,
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
            tool: Tool::None,
            crop_drag: None,
            aspect: -1.,
            retouch: Default::default(),
            masking: Default::default(),
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
    /// Per preset: the profile it names and the one it renders with instead.
    pub(super) substitutes: Vec<Option<(String, String)>>,
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
        self.region = None;
        self.navigator = None;
        self.histogram = [[0; 256]; 3];
        self.status.clear();
        self.last_fit_edge = 0;
        self.last_region = None;
        self.mode = TextureMode::Whole;
    }
    /// Textures the renderer presented into that the viewport draws.
    pub fn presented(&self) -> Vec<egui::TextureId> {
        [&self.texture, &self.region, &self.navigator]
            .into_iter()
            .flatten()
            .filter(|p| p.is_presented())
            .map(Picture::id)
            .collect()
    }
    /// Stops drawing textures the renderer presented into, once it has freed them.
    /// Not rendering again at once: a render that keeps failing would repeat.
    pub fn forget_presented(&mut self) {
        for slot in [&mut self.texture, &mut self.region, &mut self.navigator] {
            if slot.as_ref().is_some_and(Picture::is_presented) {
                *slot = None;
            }
        }
    }
}
impl ViewState {
    pub fn is(&self, tool: Tool) -> bool {
        self.tool == tool
    }
    /// Opens `tool`, or closes it when it is already open.
    pub fn toggle(&mut self, tool: Tool) {
        self.tool = if self.tool == tool { Tool::None } else { tool };
        if matches!(self.tool, Tool::Crop) {
            self.zoom100 = false;
        }
    }
    pub fn clear_document(&mut self) {
        self.zoom100 = false;
        self.zoom_anim = None;
        self.shown_rect = None;
        self.tool = Tool::None;
        self.crop_drag = None;
        self.retouch.clear_document();
        self.masking.clear_document();
        self.compare = false;
    }
}
impl PresetBrowser {
    pub fn clear_document(&mut self) {
        self.issues.clear();
        self.substitutes.clear();
        self.selected.clear();
        self.preview = None;
        self.hover = None;
    }
}

impl Document {
    pub fn full(&self) -> Option<&Arc<CameraImage>> {
        self.image.as_ref()
    }
    pub fn set_image(&mut self, full: Arc<CameraImage>) {
        self.image = Some(full);
    }
}
