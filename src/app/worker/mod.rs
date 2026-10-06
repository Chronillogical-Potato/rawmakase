use crate::{
    develop::{Recipe, Rendered},
    export::ExportOptions,
    raw::{CameraImage, Metadata},
};
use eframe::egui;
use std::{
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool, mpsc::Sender},
};

/// A loaded header can be installed as a unit before pixel development finishes.
pub struct LoadedHeader {
    pub id: u64,
    pub path: PathBuf,
    pub metadata: Metadata,
    pub recipe: Recipe,
    pub export: ExportOptions,
    pub protected: bool,
    pub status: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderStage {
    Draft,
    Fit,
    Region,
}
impl RenderStage {
    fn label(self) -> &'static str {
        match self {
            Self::Draft => "Draft • refining",
            Self::Fit => "Fit • full quality",
            Self::Region => "100% • full quality",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskKind {
    Load,
    Render(Pane),
}
/// Which view of the edit a render is for: the edit itself, or the Before beside it
/// in Lightroom's Before/After views. Each has its own render lane and caches, so an
/// edit renders only the After.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Pane {
    #[default]
    After,
    Before,
}

/// What an Auto request sets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutoKind {
    /// The six Tone sliders, as Lightroom's Auto button; white balance is kept.
    Settings,
    /// White balance alone, the WB menu's Auto.
    WhiteBalance,
}
pub enum Event {
    DialogClosed,
    /// A catalog import or open is under way, as a status line.
    CatalogWorking(String),
    CatalogReady(Result<Box<crate::app::library::Library>, String>),
    /// A folder change waiting for the user's answer.
    FolderQuestion(Box<crate::app::folder_locations::FolderQuestion>),
    Monitor(PathBuf),
    /// Files and folders chosen to import profiles or presets from.
    Import(crate::app::bulk_import::ImportKind, Vec<PathBuf>),
    Imported(Box<crate::app::bulk_import::Summary>),
    /// A Sync Settings finished.
    Synced(Box<crate::app::sync::SyncResult>),
    Profiles {
        id: u64,
        profiles: Vec<Arc<crate::camera_profiles::CameraProfile>>,
        errors: Vec<String>,
    },
    PresetLoad(PathBuf),
    /// Point Color's dropper sample for the photo loaded as `id`, taken with
    /// `sampled`: a swatch's `source`, or what to say instead.
    PointColorSample {
        id: u64,
        /// The sampling task's generation; a later sample or a put-away dropper
        /// supersedes it.
        generation: u64,
        sampled: Box<crate::develop::Recipe>,
        result: Result<[f32; 3], String>,
    },
    /// The Targeted Adjustment Tool's sample for the photo loaded as `id`, taken
    /// with `sampled` where a drag started, or what to say instead.
    TargetedSample {
        id: u64,
        /// The sampling task's generation; a later drag supersedes it.
        generation: u64,
        sampled: Box<crate::develop::Recipe>,
        result: Result<crate::develop::targeted::TargetSample, String>,
    },
    /// An Auto estimate for the photo loaded as `id`.
    Auto {
        id: u64,
        kind: AutoKind,
        result: Result<Box<crate::develop::Recipe>, String>,
    },
    /// Upright's corrections for the photo loaded as `id`, analysed from `analysed`.
    Upright {
        id: u64,
        generation: u64,
        analysed: Box<crate::develop::Recipe>,
        result: Result<Vec<[f32; 9]>, String>,
    },
    /// The Crop panel's Auto straighten angle for the photo loaded as `id`, analysed
    /// from `analysed`; None when the photo has nothing to level by.
    Straighten {
        id: u64,
        generation: u64,
        analysed: Box<crate::develop::Recipe>,
        result: Result<Option<f32>, String>,
    },
    XmpLibrary {
        scan: u64,
        library: Arc<crate::presets::Library>,
    },
    PresetSave(PathBuf),
    Header(Box<LoadedHeader>),
    Embedded {
        id: u64,
        image: image::RgbImage,
    },
    Ready {
        id: u64,
        full: Arc<CameraImage>,
        status: String,
    },
    Rendered {
        id: u64,
        pane: Pane,
        preview: Preview,
        histogram: Box<crate::develop::Histogram>,
        /// A reduced copy for the library, without overlays, when the job asked for one.
        thumbnail: Option<image::RgbImage>,
        /// The shown pixels without overlays or monitor profile, when the job asked.
        samples: Option<image::RgbImage>,
        stage: RenderStage,
        status: String,
    },
    /// The whole photo's histogram, for a render that showed a 100% region.
    Histogram {
        id: u64,
        histogram: Box<crate::develop::Histogram>,
    },
    Failed {
        id: u64,
        task: TaskKind,
        error: String,
    },
    /// The renderer recovered from a failure and no longer uses the textures its
    /// previews were presented into: stop drawing them, then drop this to free them.
    RendererReset(RetiredTextures),
    /// A batch of the export queue finished: each photo's outcome, in order.
    BatchExported {
        ticket: u64,
        outcomes: Vec<crate::export::batch::Outcome>,
    },
    /// Develop's Reference View photo, developed (half-size, then full), or why not.
    Reference {
        ticket: u64,
        result: Result<Box<ReferenceImage>, String>,
    },
}
/// Textures the renderer registered with the UI and no longer uses. Freed when
/// dropped, so the UI releases them only once it no longer draws them.
pub struct RetiredTextures {
    state: eframe::egui_wgpu::RenderState,
    ids: Vec<egui::TextureId>,
}
impl Drop for RetiredTextures {
    fn drop(&mut self) {
        let mut renderer = self.state.renderer.write();
        for id in &self.ids {
            renderer.free_texture(id);
        }
    }
}
/// A rendered preview as the viewport draws it.
pub enum Preview {
    /// Rendered on the CPU: display bytes for a texture upload.
    Pixels {
        image: Rendered,
        display_rgb: Vec<u8>,
        /// The Navigator's copy, for whole-photo views.
        navigator: Option<image::RgbImage>,
    },
    /// Presented on the GPU into textures registered with the UI's renderer.
    Texture {
        id: egui::TextureId,
        size: [usize; 2],
        navigator: Option<(egui::TextureId, [usize; 2])>,
    },
}
pub struct LoadJob {
    pub id: u64,
    pub path: PathBuf,
    pub cancel: Arc<AtomicBool>,
    /// The photo to decode ahead of time once this one is fully developed.
    pub prefetch: Option<Prefetch>,
    /// What the photo starts from when it has no edit.
    pub defaults: Arc<crate::develop::defaults::DevelopDefaults>,
    /// The demosaic of the full-size decode and of its decode-cache key.
    pub demosaic: crate::raw::Demosaic,
}
/// A photo to develop into the decode cache ahead of time, so opening it next
/// skips decoding. It has its own cancel flag: the photo on screen finishing
/// must not stop it, moving to another photo must.
pub struct Prefetch {
    pub path: PathBuf,
    pub cancel: Arc<AtomicBool>,
    pub demosaic: crate::raw::Demosaic,
}
/// What is drawn over (or instead of) the rendered photo.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Overlay {
    #[default]
    None,
    /// Visualize Spots with its threshold (0–1).
    Spots(f32),
    /// The mask at this index of the recipe's masks, tinted with this colour and
    /// opacity.
    Mask {
        index: usize,
        color: [u8; 3],
        opacity: f32,
    },
}
pub struct RenderJob {
    pub id: u64,
    pub pane: Pane,
    pub image: Arc<CameraImage>,
    pub max_edge: u32,
    pub cancel: Arc<AtomicBool>,
    pub recipe: Recipe,
    pub region: Option<[u32; 4]>,
    pub monitor: Option<PathBuf>,
    /// The clipping warnings painted over the shown pixels.
    pub clipping: crate::develop::ClipOverlay,
    /// Update the Navigator (Fit views).
    pub navigator: bool,
    /// Also produce a library thumbnail of the result.
    pub thumbnail: bool,
    /// Also return the shown pixels, for the white balance selector's loupe.
    pub samples: bool,
    pub overlay: Overlay,
    /// Presented textures the viewport draws until this job's preview arrives.
    pub drawn: Vec<egui::TextureId>,
}
fn send(tx: &Sender<Event>, ctx: &egui::Context, event: Event) {
    let _ = tx.send(event);
    ctx.request_repaint();
}

mod latest;
mod loader;
mod reference;
mod renderer;
pub use latest::Latest;
pub(crate) use latest::panic_message;
pub use loader::loader;
pub(in crate::app) use reference::{ReferenceImage, ReferenceJob, Resolution, reference_loader};
pub(super) use renderer::{RenderBackend, renderer_with_backend};
pub use renderer::{Renderer, renderer};
