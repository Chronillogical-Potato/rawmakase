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
    pub files: Vec<PathBuf>,
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
    Render,
}

pub enum Event {
    DialogClosed,
    /// A catalog import or open is under way, as a status line.
    CatalogWorking(String),
    CatalogReady(Result<Box<crate::app::library::Library>, String>),
    Open(PathBuf),
    Monitor(PathBuf),
    /// Files and folders chosen to import profiles or presets from.
    Import(crate::app::bulk_import::ImportKind, Vec<PathBuf>),
    Imported(Box<crate::app::bulk_import::Summary>),
    Profiles {
        id: u64,
        profiles: Vec<Arc<crate::camera_profiles::CameraProfile>>,
        errors: Vec<String>,
    },
    PresetLoad(PathBuf),
    XmpLibrary(Arc<crate::presets::Library>),
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
    Thumbnail {
        id: u64,
        path: PathBuf,
        image: image::RgbImage,
    },
    Rendered {
        id: u64,
        preview: Preview,
        histogram: Box<[[u32; 256]; 3]>,
        /// A reduced copy for the library, without overlays, when the job asked for one.
        thumbnail: Option<image::RgbImage>,
        stage: RenderStage,
        status: String,
    },
    /// The whole photo's histogram, for a render that showed a 100% region.
    Histogram {
        id: u64,
        histogram: Box<[[u32; 256]; 3]>,
    },
    Failed {
        id: u64,
        task: TaskKind,
        error: String,
    },
    Exported(String),
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
    pub catalog: bool,
    pub id: u64,
    pub path: PathBuf,
    pub cancel: Arc<AtomicBool>,
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
    pub image: Arc<CameraImage>,
    pub max_edge: u32,
    pub cancel: Arc<AtomicBool>,
    pub recipe: Recipe,
    pub region: Option<[u32; 4]>,
    pub monitor: Option<PathBuf>,
    pub clipping: bool,
    /// Update the Navigator (Fit views).
    pub navigator: bool,
    /// Also produce a library thumbnail of the result.
    pub thumbnail: bool,
    pub overlay: Overlay,
}
fn send(tx: &Sender<Event>, ctx: &egui::Context, event: Event) {
    let _ = tx.send(event);
    ctx.request_repaint();
}

mod latest;
mod loader;
mod renderer;
pub use latest::Latest;
pub use loader::loader;
pub use renderer::renderer;
pub(super) use renderer::{RenderBackend, renderer_with_backend};
