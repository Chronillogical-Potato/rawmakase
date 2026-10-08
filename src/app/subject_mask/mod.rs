//! Select Subject and Select Background (docs/subject-and-background-masks.md): a
//! model finds the photo's salient foreground, and the result becomes a mask that
//! edits like any other. Everything here is the app's side of that: what a request
//! captures, when its result may still be applied, and what the drawer says
//! meanwhile. The model and its runtime are `rawmakase-inference`'s; installing the
//! model is `models`; the thread that runs it is `worker`.
mod models;
mod ui;
mod worker;

use super::Editor;
use super::task::{Stopping, Task};
use crate::model::masks::{
    BITMAP_SAMPLING, BitmapMask, BitmapSource, FEATURE_SUBJECT, MAX_COMPONENTS, MAX_GROUPS,
    MaskComponent, MaskGroup, MaskOp, MaskShape,
};
use crate::model::recipe::Recipe;
use rawmakase_inference::{Point, Prompt as ModelPrompt};
use std::hash::{Hash, Hasher};

pub(super) use models::Installer;

/// Which of the two selections a request makes. Background is the same coverage with
/// the component inverted, so adding a brush to it still adds coverage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Feature {
    Subject,
    Background,
}
impl Feature {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Subject => "Subject",
            Self::Background => "Background",
        }
    }
    fn invert(self) -> bool {
        self == Self::Background
    }
}

/// What a finished selection does to the edit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Target {
    /// A new mask holding the selection.
    NewMask,
    /// A component of the selected mask, combined with `op`.
    Component { mask: usize, op: MaskOp },
    /// Replace the raster of one generated component, keeping everything else.
    Regenerate { mask: usize, component: usize },
}

/// A request, as it was made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Request {
    pub(super) feature: Feature,
    pub(super) target: Target,
}

/// Why a selection did not make a mask; the drawer says so with a way to try again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Failure {
    Cancelled,
    /// The model found nothing; no mask is made.
    NoSubject,
    /// The model file is not installed (or was removed meanwhile).
    ModelMissing,
    /// The inference runtime could not be loaded or has no support here.
    RuntimeUnavailable(String),
    Failed(String),
}
impl Failure {
    fn message(&self) -> String {
        match self {
            Self::Cancelled => "Cancelled".into(),
            Self::NoSubject => "Nothing selected there; click on the subject itself".into(),
            Self::ModelMissing => "The selection model is not installed".into(),
            Self::RuntimeUnavailable(why) => format!("Selection is unavailable here: {why}"),
            Self::Failed(why) => format!("Selection failed: {why}"),
        }
    }
}

/// A raster a selection produced, registered in the mask asset store.
#[derive(Debug)]
pub(crate) struct Generated {
    pub(crate) id: String,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) source: BitmapSource,
}

/// A selection's outcome, tagged with what it was started for.
pub(crate) struct Done {
    /// The photo's load, and the task generation, it was started under.
    pub(crate) load: u64,
    pub(crate) generation: u64,
    pub(crate) result: Result<Generated, Failure>,
}

/// What must still hold when a result arrives for it to be applied.
struct Guard {
    /// The masks' structure: how many, with which components. Any addition, removal,
    /// reordering or replacement (Undo, a snapshot) changes it; sliders, names and
    /// visibility do not.
    structure: u64,
    /// The spots and red eye the model saw. Different ones select different content.
    content: u64,
    /// For a regeneration, the component's shape when it was asked for.
    expected: Option<MaskShape>,
}
impl Guard {
    fn of(recipe: &Recipe, request: Request) -> Self {
        let expected = match request.target {
            Target::Regenerate { mask, component } => recipe
                .masks
                .get(mask)
                .and_then(|m| m.components.get(component))
                .map(|c| c.shape.clone()),
            _ => None,
        };
        Self {
            structure: structure(&recipe.masks),
            content: content(recipe),
            expected,
        }
    }
    fn holds(&self, recipe: &Recipe, request: Request) -> bool {
        let same_shape = match request.target {
            Target::Regenerate { mask, component } => {
                recipe
                    .masks
                    .get(mask)
                    .and_then(|m| m.components.get(component))
                    .map(|c| &c.shape)
                    == self.expected.as_ref()
                    && self.expected.is_some()
            }
            _ => true,
        };
        same_shape && self.structure == structure(&recipe.masks) && self.content == content(recipe)
    }
}
/// The masks' structure, not their values.
fn structure(masks: &[MaskGroup]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    masks.len().hash(&mut h);
    for g in masks {
        g.components.len().hash(&mut h);
        for c in &g.components {
            c.shape.kind().hash(&mut h);
            if let MaskShape::Bitmap(b) = &c.shape {
                b.id.hash(&mut h);
            }
        }
    }
    h.finish()
}
/// What the model is given besides pixels the settings do not change: spots and red
/// eye, which alter the content to select.
fn content(recipe: &Recipe) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_string(&(&recipe.retouch, &recipe.red_eye))
        .unwrap_or_default()
        .hash(&mut h);
    h.finish()
}

struct Pending {
    request: Request,
    guard: Guard,
}

/// What the drawer asks the user before a selection can run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Prompt {
    /// The model has to be downloaded or imported.
    Model(Request),
    /// The catalog has to be upgraded to keep the result.
    Upgrade(Request),
}

/// A selection being aimed: the clicks and box so far. The first one makes the mask;
/// each further one refines that same mask until the user is done.
pub(super) struct Prompting {
    pub(super) request: Request,
    pub(super) points: Vec<Point>,
    pub(super) bounds: Option<[f32; 4]>,
    /// The mask and component the first result made (or the one being regenerated).
    applied: Option<(usize, usize)>,
}

#[derive(Default)]
pub(super) struct Selection {
    /// Waiting for clicks on the photo.
    pub(super) prompting: Option<Prompting>,
    /// Where a drag for a box began, in image space.
    pub(super) drag_from: Option<[f32; 2]>,
    /// The running selection's generation; a newer request or a new photo supersedes.
    task: Task,
    pending: Option<Pending>,
    /// What the last attempt ended in, for the drawer, until the next.
    failure: Option<(Request, Failure)>,
    prompt: Option<Prompt>,
    worker: worker::Worker,
    pub(super) models: Installer,
    /// A catalog upgrade under way, and the request it unblocks.
    upgrading: Option<(u64, Request)>,
    upgrade_task: Task,
}

impl Selection {
    /// Whether a selection is running.
    pub(super) fn running(&self) -> Option<Request> {
        self.pending.as_ref().map(|p| p.request)
    }
    /// Stops the running selection, whatever it is for (a new photo, Cancel).
    pub(super) fn cancel(&mut self) {
        self.task.invalidate();
        self.pending = None;
    }
    /// Stops aiming (Done, Escape, a new photo).
    pub(super) fn end_prompting(&mut self) {
        self.prompting = None;
        self.drag_from = None;
    }
    /// The photo changed: nothing asked for the previous one applies.
    pub(super) fn clear_document(&mut self) {
        self.cancel();
        self.end_prompting();
        self.failure = None;
        self.prompt = None;
    }
    /// Workers to wait for when quitting: inference is asked to stop and handed to the
    /// shared deadline, never joined without one.
    pub(super) fn stop(&mut self) -> Vec<Stopping> {
        self.cancel();
        self.end_prompting();
        let mut stopping = vec![self.worker.stop()];
        stopping.extend(self.models.stop());
        stopping
    }
}

impl Editor {
    /// Why Subject and Background cannot be made now, for the drawer to say.
    pub(super) fn selection_unavailable(&self) -> Option<&'static str> {
        if self.library.is_none() || self.document.catalog_photo.is_none() {
            return Some("Add this photo to a catalog to use AI masks");
        }
        if self.document.full().is_none() {
            return Some("Available once the photo has finished developing");
        }
        if self.document.edit.save_state().is_protected() {
            return Some("This edit is protected and cannot be changed here");
        }
        None
    }
    /// Starts a selection, or asks what it needs first: the model, an upgraded
    /// catalog.
    pub(super) fn request_selection(&mut self, request: Request) {
        if let Some(why) = self.selection_unavailable() {
            self.status = why.into();
            return;
        }
        let masks = &self.document.edit.recipe().masks;
        let room = match request.target {
            Target::NewMask => masks.len() < MAX_GROUPS,
            Target::Component { mask, .. } => masks
                .get(mask)
                .is_some_and(|m| m.components.len() < MAX_COMPONENTS),
            Target::Regenerate { .. } => true,
        };
        if !room {
            self.status = format!("A photo holds up to {MAX_GROUPS} masks");
            return;
        }
        let upgraded = self
            .library
            .as_ref()
            .and_then(|l| l.session.catalog.supports_raster_masks().ok())
            .unwrap_or(false);
        if !upgraded {
            self.selection.prompt = Some(Prompt::Upgrade(request));
            return;
        }
        if !self.selection.models.installed() {
            self.selection.prompt = Some(Prompt::Model(request));
            return;
        }
        self.selection.prompt = None;
        self.selection.failure = None;
        self.selection.prompting = Some(Prompting {
            request,
            points: Vec::new(),
            bounds: None,
            applied: match request.target {
                Target::Regenerate { mask, component } => Some((mask, component)),
                _ => None,
            },
        });
        self.view.tool = super::state::Tool::Mask;
        self.status = format!(
            "Click the {} on the photo, or drag a box around it",
            if request.feature == Feature::Background {
                "subject to leave out of the background"
            } else {
                "subject"
            }
        );
    }
    /// A click on the photo while aiming: a point on the object (or, with `positive`
    /// false, on something to leave out). Starts or refines the selection.
    pub(super) fn prompt_click(&mut self, at: [f32; 2], positive: bool) {
        let Some(p) = &mut self.selection.prompting else {
            return;
        };
        if self.selection.pending.is_some()
            || !(0. ..=1.).contains(&at[0])
            || !(0. ..=1.).contains(&at[1])
        {
            return;
        }
        // A negative point means nothing without something to be negative about.
        if !positive && p.points.iter().all(|q| !q.positive) && p.bounds.is_none() {
            return;
        }
        if p.points.len() < rawmakase_inference::process::MAX_POINTS {
            p.points.push(Point {
                x: at[0],
                y: at[1],
                positive,
            });
        }
        self.run_prompt();
    }
    /// A box dragged on the photo while aiming (image space corners).
    pub(super) fn prompt_box(&mut self, a: [f32; 2], b: [f32; 2]) {
        let Some(p) = &mut self.selection.prompting else {
            return;
        };
        let clamp = |v: f32| v.clamp(0., 1.);
        let bounds = [
            clamp(a[0].min(b[0])),
            clamp(a[1].min(b[1])),
            clamp(a[0].max(b[0])),
            clamp(a[1].max(b[1])),
        ];
        if self.selection.pending.is_some()
            || bounds[2] - bounds[0] < 0.01
            || bounds[3] - bounds[1] < 0.01
        {
            return;
        }
        p.bounds = Some(bounds);
        self.run_prompt();
    }
    /// Forgets the clicks and box, so the next click starts the selection over (the
    /// mask made so far is replaced by it).
    pub(super) fn clear_prompt(&mut self) {
        if let Some(p) = &mut self.selection.prompting {
            p.points.clear();
            p.bounds = None;
        }
    }
    /// Runs the model on the clicks and box so far.
    fn run_prompt(&mut self) {
        let Some(p) = &self.selection.prompting else {
            return;
        };
        let prompt = ModelPrompt {
            points: p.points.clone(),
            bounds: p.bounds,
        };
        let request = Request {
            feature: p.request.feature,
            target: match p.applied {
                Some((mask, component)) => Target::Regenerate { mask, component },
                None => p.request.target,
            },
        };
        self.selection.failure = None;
        self.start_selection(request, prompt);
    }
    fn start_selection(&mut self, request: Request, prompt: ModelPrompt) {
        let Some(image) = self.document.full().cloned() else {
            return;
        };
        let Some(model) = self.selection.models.path() else {
            self.selection.end_prompting();
            self.selection.prompt = Some(Prompt::Model(request));
            return;
        };
        let (generation, cancel) = self.selection.task.start();
        let recipe = self.document.edit.recipe().clone();
        self.selection.pending = Some(Pending {
            request,
            guard: Guard::of(&recipe, request),
        });
        self.selection.worker.submit(
            worker::Job {
                load: self.load.id(),
                generation,
                cancel,
                image,
                recipe,
                model,
                prompt,
            },
            self.tx.clone(),
            self.context.clone(),
        );
    }
    pub(super) fn cancel_selection(&mut self) {
        self.selection.cancel();
        self.status = "Selection cancelled".into();
    }
    /// A selection finished: apply it if it is still the one wanted and the edit it
    /// was made for still stands, else drop it.
    pub(super) fn selection_done(&mut self, done: Done) {
        if done.load != self.load.id() || done.generation != self.selection.task.id() {
            return;
        }
        self.selection.task.finish(done.generation);
        let Some(pending) = self.selection.pending.take() else {
            return;
        };
        let request = pending.request;
        let generated = match done.result {
            Ok(generated) => generated,
            Err(Failure::Cancelled) => return,
            Err(failure) => {
                self.status = failure.message();
                self.selection.failure = Some((request, failure));
                return;
            }
        };
        if !pending.guard.holds(self.document.edit.recipe(), request) {
            self.status = "The masks changed while selecting; nothing was added".into();
            return;
        }
        // The rasters this would add, with every other the edit already refers to.
        let ids: Vec<&str> = self
            .document
            .edit
            .recipe()
            .mask_asset_ids()
            .chain([generated.id.as_str()])
            .collect();
        let bytes = crate::storage::mask_assets::decoded_bytes(ids);
        if bytes > crate::storage::mask_assets::EDIT_BYTES {
            self.status = "This photo's masks use too much memory for another selection".into();
            return;
        }
        let masks = self.document.edit.recipe().masks.len();
        let shape = MaskShape::Bitmap(BitmapMask {
            id: generated.id,
            width: generated.width,
            height: generated.height,
            sampling: BITMAP_SAMPLING,
            source: Some(generated.source),
        });
        let name = match request.target {
            Target::Regenerate { .. } => format!("Refine {}", request.feature.name()),
            _ => format!("Select {}", request.feature.name()),
        };
        let selected = self.change_edit(
            Some(super::history::Step::new(name, "")),
            |r| match request.target {
                Target::NewMask => {
                    let mut component = MaskComponent::new(shape);
                    component.invert = request.feature.invert();
                    r.masks.push(MaskGroup {
                        name: request.feature.name().into(),
                        components: vec![component],
                        ..Default::default()
                    });
                    Some((r.masks.len() - 1, 0))
                }
                Target::Component { mask, op } => {
                    let group = r.masks.get_mut(mask)?;
                    let mut component = MaskComponent::new(shape);
                    component.op = op;
                    component.invert = request.feature.invert();
                    group.components.push(component);
                    Some((mask, group.components.len() - 1))
                }
                Target::Regenerate { mask, component } => {
                    let c = r.masks.get_mut(mask)?.components.get_mut(component)?;
                    c.shape = shape;
                    Some((mask, component))
                }
            },
        );
        debug_assert!(masks <= MAX_GROUPS);
        if let Some((mask, component)) = selected {
            if let Some(p) = &mut self.selection.prompting {
                p.applied = Some((mask, component));
            }
            self.view.masking.select_after_selection(mask, component);
            self.status = format!("{} selected", request.feature.name());
        }
    }
    /// The installer finished or was cancelled; a request waiting for the model goes
    /// on.
    pub(super) fn model_installed(&mut self, result: Result<(), String>) {
        match result {
            Ok(()) => {
                self.status = "Selection model installed".into();
                if let Some(Prompt::Model(request)) = self.selection.prompt.take() {
                    self.request_selection(request);
                }
            }
            Err(why) => self.status = format!("Model not installed: {why}"),
        }
    }
    /// Asks the installer to download the model (the prompt's Download) or to import a
    /// file the user chose.
    pub(super) fn install_model(&mut self, import: Option<std::path::PathBuf>) {
        self.selection
            .models
            .install(import, self.tx.clone(), self.context.clone());
    }
    /// Upgrades the open catalog on a thread of its own, after the edit is saved.
    pub(super) fn upgrade_catalog(&mut self, request: Request) {
        if self.selection.upgrading.is_some() {
            return;
        }
        if !self.flush() {
            return;
        }
        let Some(location) = self
            .library
            .as_ref()
            .map(|l| l.session.catalog.location().clone())
        else {
            return;
        };
        let (generation, _) = self.selection.upgrade_task.start();
        self.selection.upgrading = Some((generation, request));
        let (tx, ctx) = (self.tx.clone(), self.context.clone());
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(|| {
                crate::catalog::Catalog::open(&location)
                    .and_then(|mut c| c.upgrade_for_raster_masks())
                    .map_err(|e| format!("{e:#}"))
            })
            .unwrap_or_else(|_| Err("the upgrade stopped unexpectedly".into()));
            let _ = tx.send(super::worker::Event::CatalogUpgraded { generation, result });
            ctx.request_repaint();
        });
    }
    pub(super) fn catalog_upgraded(
        &mut self,
        generation: u64,
        result: Result<Option<std::path::PathBuf>, String>,
    ) {
        let Some((current, request)) = self.selection.upgrading else {
            return;
        };
        if generation != current {
            return;
        }
        self.selection.upgrading = None;
        self.selection.upgrade_task.finish(generation);
        match result {
            Ok(backup) => {
                if let Some(backup) = backup {
                    self.status = format!("Catalog upgraded · backup: {}", backup.display());
                }
                self.selection.prompt = None;
                self.request_selection(request);
            }
            Err(why) => self.status = format!("Catalog not upgraded: {why}"),
        }
    }
}

/// Where a mask's raster came from, for the component settings.
pub(super) fn source_text(b: &BitmapMask) -> String {
    match b.source.as_ref().map(|s| s.feature.as_str()) {
        Some(FEATURE_SUBJECT) => "Selected by the subject model".into(),
        _ => "A raster mask".into(),
    }
}

#[cfg(test)]
mod tests;
