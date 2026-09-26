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
    CatalogReady(Result<Box<crate::app::library::Library>, String>),
    Open(PathBuf),
    ExportPath(PathBuf),
    Monitor(PathBuf),
    CameraProfile(Vec<PathBuf>),
    LensProfiles(Vec<PathBuf>),
    Profiles {
        id: u64,
        profiles: Vec<Arc<crate::camera_profiles::CameraProfile>>,
        errors: Vec<String>,
    },
    PresetLoad(PathBuf),
    XmpImport(PathBuf),
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
        draft: Arc<CameraImage>,
        status: String,
    },
    Thumbnail {
        id: u64,
        path: PathBuf,
        image: image::RgbImage,
    },
    Rendered {
        id: u64,
        image: Rendered,
        display_rgb: Vec<u8>,
        stage: RenderStage,
        status: String,
    },
    Failed {
        id: u64,
        task: TaskKind,
        error: String,
    },
    Exported(String),
}
pub struct LoadJob {
    pub catalog: bool,
    pub id: u64,
    pub path: PathBuf,
    pub cancel: Arc<AtomicBool>,
}
pub struct RenderJob {
    pub id: u64,
    pub image: Arc<CameraImage>,
    pub draft: Arc<CameraImage>,
    pub max_edge: u32,
    pub cancel: Arc<AtomicBool>,
    pub recipe: Recipe,
    pub region: Option<[u32; 4]>,
    pub monitor: Option<PathBuf>,
    pub clipping: bool,
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

/// Interactive feedback is intentionally smaller than the final physical-pixel fit.
const DRAFT_EDGE: u32 = 1024;
