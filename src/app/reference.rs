//! Lightroom's Reference View in Develop (Shift+R): another catalog photo shown
//! beside the one being edited, left/right or top/bottom, to match its look.
//!
//! The reference is set from the filmstrip (drag a photo onto the Reference side, or
//! right-click › Set as Reference Photo) and stays while moving between photos.
//! Leaving Develop clears it unless it is locked, as in Lightroom. It shows with its
//! own saved edit, framed by its own crop, and is never edited: panels and keys act
//! on the Active photo. It has its own zoom, Fit or 100%, as Lightroom's has. It is
//! decoded on its own worker (half size first, for Fit, then in full for 100%) and
//! rendered in Before's lane of the renderer, so it never holds up the edit.
use super::Editor;
use super::before_after::{Axis, Compare, Pane, Panes, badge};
use super::navigator::Zoom;
use super::worker::{ReferenceImage, ReferenceJob, Resolution};
use crate::app::library::DraggedPhoto;
use crate::app::theme;
use crate::{
    develop::{Geometry, Recipe},
    raw::CameraImage,
};
use eframe::egui::{self, Color32, Pos2, Rect, Vec2};
use std::sync::Arc;

/// Reference View's state: the reference photo, its lock, layout and zoom, and the
/// photo as last developed.
pub(super) struct ReferenceView {
    /// The catalog photo shown as the reference.
    pub(super) photo: Option<i64>,
    /// Kept when leaving Develop, as Lightroom's lock in the toolbar.
    pub(super) locked: bool,
    /// Where the reference sits: left or on top. Shift+R shows the last one used.
    pub(super) layout: Axis,
    /// The reference's own Fit or 100%, and where.
    pub(super) zoom: Zoom,
    /// The reference as developed, once it is.
    loaded: Option<Loaded>,
    /// The photo and edit being developed, and its generation.
    pending: Option<(i64, u64)>,
    load: super::task::Task,
    /// Why the reference could not be shown.
    pub(super) error: Option<String>,
}
impl Default for ReferenceView {
    fn default() -> Self {
        Self {
            photo: None,
            locked: false,
            layout: Axis::LeftRight,
            zoom: Zoom::default(),
            loaded: None,
            pending: None,
            load: Default::default(),
            error: None,
        }
    }
}
/// The reference photo as developed.
struct Loaded {
    photo: i64,
    /// Its edit stamp then: a later edit develops it again.
    stamp: u64,
    image: Arc<CameraImage>,
    recipe: Recipe,
}
/// What the Reference side renders: the photo and its edit.
pub(super) struct ReferenceSide {
    pub(super) image: Arc<CameraImage>,
    pub(super) recipe: Recipe,
}

impl ReferenceView {
    /// No reference: the side shows how to set one.
    fn clear(&mut self) {
        self.photo = None;
        self.loaded = None;
        self.pending = None;
        self.load.invalidate();
        self.error = None;
        self.zoom = Zoom::default();
    }
    /// Whether the reference is still being developed.
    pub(super) fn loading(&self) -> bool {
        self.load.is_running()
    }
}

impl Editor {
    /// Reference View is shown.
    pub(super) fn reference_view(&self) -> bool {
        self.view.compare.reference()
    }
    /// Shift+R: Reference View in its last layout, or back to the edit alone.
    pub(super) fn toggle_reference_view(&mut self) {
        let view = self
            .view
            .compare
            .toggled(Compare::Reference(self.reference.layout));
        self.set_compare(view);
    }
    /// Reference View with the reference left or on top.
    pub(super) fn set_reference_layout(&mut self, layout: Axis) {
        self.reference.layout = layout;
        self.set_compare(Compare::Reference(layout));
    }
    /// Makes catalog photo `id` the reference and shows Reference View, as dropping
    /// it on the Reference side or Set as Reference Photo does.
    pub(super) fn set_reference(&mut self, id: i64) {
        let source = self.library.as_ref().and_then(|l| l.develop_source(id));
        let Some(source) = source else {
            return;
        };
        if let Err(refusal) = source {
            self.status = format!("Can't be the reference photo: {}", refusal.detail());
            return;
        }
        if self.reference.photo != Some(id) {
            self.reference.clear();
            self.reference.photo = Some(id);
            self.preview.before.clear();
        }
        if !self.reference_view() {
            self.set_compare(Compare::Reference(self.reference.layout));
        }
        self.load_reference();
    }
    /// Develops the reference photo when it is not, or its edit changed since; the
    /// photo being edited needs nothing, as it is already open.
    pub(super) fn load_reference(&mut self) {
        let Some(id) = self.reference.photo else {
            return;
        };
        if self.document.catalog_photo == Some(id) {
            return;
        }
        let source = self.library.as_ref().and_then(|l| l.develop_source(id));
        let source = match source {
            // No longer in the catalog, e.g. a virtual copy removed.
            None => return self.clear_reference(),
            Some(Err(refusal)) => {
                self.reference.error = Some(refusal.detail());
                return;
            }
            Some(Ok(source)) => source,
        };
        let wanted = (id, source.stamp);
        let developed = self
            .reference
            .loaded
            .as_ref()
            .is_some_and(|l| (l.photo, l.stamp) == wanted);
        if developed || self.reference.pending == Some(wanted) {
            return;
        }
        let (ticket, cancel) = self.reference.load.start();
        self.reference.pending = Some(wanted);
        self.reference.error = None;
        self.reference_loader.submit(ReferenceJob {
            ticket,
            path: source.path,
            edit: source.edit,
            cancel,
        });
    }
    /// The reference photo developed, or why not.
    pub(super) fn reference_ready(
        &mut self,
        ticket: u64,
        result: Result<Box<ReferenceImage>, String>,
    ) {
        if ticket != self.reference.load.id() {
            return;
        }
        let Some((photo, stamp)) = self.reference.pending else {
            return;
        };
        match result {
            Ok(developed) => {
                if developed.resolution == Resolution::Full {
                    self.reference.load.finish(ticket);
                    self.reference.pending = None;
                }
                self.reference.loaded = Some(Loaded {
                    photo,
                    stamp,
                    image: developed.image,
                    recipe: developed.recipe,
                });
            }
            Err(error) => {
                self.reference.load.finish(ticket);
                self.reference.pending = None;
                self.status = format!("Reference photo: {error}");
                self.reference.error = Some(error);
            }
        }
        self.schedule_before();
    }
    /// What the Reference side shows: the reference as developed, or the photo being
    /// edited, live, when it is the reference.
    pub(super) fn reference_side(&self) -> Option<ReferenceSide> {
        let id = self.reference.photo?;
        if self.document.catalog_photo == Some(id) {
            return Some(ReferenceSide {
                image: self.document.full()?.clone(),
                recipe: self.document.recipe.clone(),
            });
        }
        let loaded = self.reference.loaded.as_ref().filter(|l| l.photo == id)?;
        Some(ReferenceSide {
            image: loaded.image.clone(),
            recipe: loaded.recipe.clone(),
        })
    }
    /// No reference photo any more; Reference View, if shown, waits for another.
    pub(super) fn clear_reference(&mut self) {
        self.reference.clear();
        self.preview.before.clear();
    }
    /// Leaving Develop: the reference goes unless locked, as in Lightroom, and the
    /// edit shows alone on return.
    pub(super) fn left_develop(&mut self) {
        if self.reference_view() {
            self.view.compare = Compare::Off;
            self.preview.before.clear();
        }
        if !self.reference.locked && self.reference.photo.is_some() {
            self.clear_reference();
        }
    }
}

impl Editor {
    /// Draws the Reference side: the reference photo at its own zoom, a stand-in
    /// while it is developed, or how to set one. Returns where the photo is drawn.
    pub(super) fn reference_pane_ui(
        &self,
        ui: &egui::Ui,
        panes: &Panes,
        pane: Pane,
    ) -> Option<Rect> {
        badge(ui, panes.after.clip, "Active");
        let rect = self.reference_rect(ui, pane);
        match (rect, self.reference.photo) {
            (Some((rect, g)), _) => self.paint_second_side(ui, pane, rect, &g),
            (None, Some(id)) => self.reference_stand_in(ui, pane, id),
            (None, None) => {
                let text = "Drag a photo from the filmstrip here,\nor right-click one and choose\nSet as Reference Photo";
                ui.painter().with_clip_rect(pane.clip).text(
                    pane.clip.center(),
                    egui::Align2::CENTER_CENTER,
                    text,
                    egui::FontId::proportional(13.),
                    theme::gray(150),
                );
            }
        }
        if let Some(error) = &self.reference.error {
            ui.painter().with_clip_rect(pane.clip).text(
                pane.clip.center_bottom() - Vec2::new(0., 24.),
                egui::Align2::CENTER_CENTER,
                error,
                egui::FontId::proportional(12.),
                theme::gray(170),
            );
        }
        badge(ui, pane.clip, "Reference");
        rect.map(|(rect, _)| rect)
    }
    /// Where the reference photo is drawn in `pane` at its own zoom, and its
    /// geometry, once it is developed.
    fn reference_rect(&self, ui: &egui::Ui, pane: Pane) -> Option<(Rect, Geometry)> {
        let side = self.reference_side()?;
        let g = Geometry::new(&side.image, &side.recipe, 0);
        let size = Vec2::new(g.width as f32, g.height as f32);
        let ppp = ui.ctx().pixels_per_point();
        Some((self.reference.zoom.photo_rect(pane.area, size, ppp), g))
    }
    /// The reference's Library preview, fitted, while it is developed.
    fn reference_stand_in(&self, ui: &egui::Ui, pane: Pane, id: i64) {
        let texture = self.library.as_ref().and_then(|l| l.thumbnail(id));
        if let Some(texture) = texture {
            let size = texture.size_vec2();
            let k = (pane.area.width() / size.x).min(pane.area.height() / size.y);
            let rect = Rect::from_center_size(pane.area.center(), size * k);
            let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1., 1.));
            ui.painter()
                .with_clip_rect(pane.clip)
                .image(texture.id(), rect, uv, Color32::WHITE);
        }
        if self.reference.loading() {
            let at = Rect::from_center_size(pane.clip.center(), Vec2::splat(18.));
            egui::Spinner::new().size(18.).paint_at(ui, at);
        }
    }
    /// Clicks, drags and drops on the Reference side: a click zooms the reference
    /// alone between Fit and 100%, a drag pans it, and a photo dragged from the
    /// filmstrip becomes the reference. Returns whether the pointer is the
    /// reference's, so the Active side leaves it alone.
    pub(super) fn reference_pointer(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        panes: &Panes,
        photo: Option<Rect>,
    ) -> bool {
        let Some(pane) = panes.before.filter(|_| self.reference_view()) else {
            return false;
        };
        let drop = ui.interact(
            pane.clip,
            ui.id().with("reference-drop"),
            egui::Sense::hover(),
        );
        if drop.dnd_hover_payload::<DraggedPhoto>().is_some() {
            ui.painter().rect_stroke(
                pane.clip.shrink(2.),
                0.,
                egui::Stroke::new(2., theme::accent()),
                egui::StrokeKind::Inside,
            );
        }
        if let Some(dropped) = drop.dnd_release_payload::<DraggedPhoto>() {
            self.set_reference(dropped.0);
            return true;
        }
        let origin = ui.input(|i| i.pointer.press_origin());
        let ours = origin.is_some_and(|p| pane.clip.contains(p) && !panes.after.clip.contains(p));
        if !ours {
            return false;
        }
        let (Some(rect), Some(side)) = (photo, self.reference_side()) else {
            return true;
        };
        let g = Geometry::new(&side.image, &side.recipe, 0);
        let zoom = &mut self.reference.zoom;
        if zoom.on && response.dragged() {
            let delta = ui.input(|i| i.pointer.delta());
            zoom.pan[0] = (zoom.pan[0] - delta.x / rect.width()).clamp(0., 1.);
            zoom.pan[1] = (zoom.pan[1] - delta.y / rect.height()).clamp(0., 1.);
        }
        if response.clicked()
            && !response.double_clicked()
            && let Some(pos) = response.interact_pointer_pos()
            && rect.contains(pos)
        {
            let size = Vec2::new(g.width as f32, g.height as f32);
            let ppp = ui.ctx().pixels_per_point();
            zoom.toggle_at(pos, rect, pane.area, size, ppp);
        }
        self.schedule_before();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::worker::Event;
    use eframe::egui;
    use std::path::Path;

    /// A catalog of two copies of the synthetic chart, the second edited, opened
    /// in an editor with the first photo open.
    struct Fixture {
        _dir: tempfile::TempDir,
        editor: Editor,
        open: i64,
        other: i64,
    }
    fn fixture() -> anyhow::Result<Fixture> {
        let dir = tempfile::tempdir()?;
        let photos = dir.path().join("photos");
        std::fs::create_dir(&photos)?;
        let chart =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/charts/synthetic-d65.dng");
        for name in ["a.dng", "b.dng"] {
            std::fs::copy(&chart, photos.join(name))?;
        }
        let catalog = dir.path().join("test.rawmakase");
        let mut c = crate::catalog::Catalog::create(&catalog)?;
        c.add_folder(&photos)?;
        drop(c);
        let ctx = egui::Context::default();
        let library = crate::app::library::Library::load(&catalog, ctx.clone())?;
        let id = |name: &str| {
            library
                .photos
                .iter()
                .find(|p| p.filename == name)
                .unwrap()
                .id
        };
        let (open, other) = (id("a.dng"), id("b.dng"));
        let edit = Recipe {
            exposure: 1.5,
            ..Default::default()
        };
        library.catalog.save_edit(
            other,
            &photos.join("b.dng"),
            &edit,
            &Default::default(),
            crate::catalog::HistoryUpdate::Keep,
        )?;
        let mut editor = Editor::with_context(&ctx, None, crate::storage::Session::default(), None);
        editor.library = Some(Box::new(library));
        editor.document.catalog_photo = Some(open);
        editor.document.path = Some(photos.join("a.dng"));
        editor.view.viewport = eframe::egui::Vec2::new(200., 150.);
        Ok(Fixture {
            _dir: dir,
            editor,
            open,
            other,
        })
    }
    /// Takes the editor's events until the reference is developed in full.
    fn wait_for_reference(editor: &mut Editor) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while editor.reference.loading() {
            assert!(
                std::time::Instant::now() < deadline,
                "reference not developed"
            );
            match editor.rx.recv_timeout(std::time::Duration::from_secs(60)) {
                Ok(Event::Reference { ticket, result }) => editor.reference_ready(ticket, result),
                Ok(_) => {}
                Err(e) => panic!("{e}"),
            }
        }
    }

    #[test]
    fn the_reference_shows_its_own_edit_and_edits_reach_the_active_photo_only() -> anyhow::Result<()>
    {
        // The fixture keeps its folder for as long as it lives.
        let mut fixture = fixture()?;
        let (open, other) = (fixture.open, fixture.other);
        let editor = &mut fixture.editor;
        editor.view.tool = super::super::state::Tool::Mask;
        editor.set_reference(other);
        // Setting a reference shows Reference View, which closes tools.
        assert_eq!(editor.view.compare, Compare::Reference(Axis::LeftRight));
        assert_eq!(editor.view.tool, super::super::state::Tool::None);
        assert!(editor.reference.loading());
        wait_for_reference(editor);
        // Developed in full, for 100%.
        let side = editor.reference_side().unwrap();
        assert_eq!(side.recipe.exposure, 1.5);
        // Editing changes the Active photo, never the reference.
        editor.document.recipe.exposure = -0.5;
        assert_eq!(editor.reference_side().unwrap().recipe.exposure, 1.5);
        // The reference renders in Before's lane, from its own photo.
        editor.schedule_before();
        assert!(editor.preview.before.task.is_running());
        // Opening the reference photo itself keeps it, shown live as it is edited.
        let image = editor.reference_side().unwrap().image;
        editor.document.reset(Some(other));
        editor.document.set_image(image);
        editor.load_reference();
        assert!(!editor.reference.loading());
        editor.document.recipe.exposure = 0.25;
        assert_eq!(editor.reference_side().unwrap().recipe.exposure, 0.25);
        editor.document.reset(Some(open));
        assert_eq!(editor.reference_side().unwrap().recipe.exposure, 1.5);
        // An edit saved to it since develops it again.
        let library = editor.library.as_ref().unwrap();
        let path = library.photo(other).unwrap().path.clone();
        let edit = Recipe {
            exposure: -1.,
            ..Default::default()
        };
        library.catalog.save_edit(
            other,
            &path,
            &edit,
            &Default::default(),
            crate::catalog::HistoryUpdate::Keep,
        )?;
        editor.load_reference();
        wait_for_reference(editor);
        assert_eq!(editor.reference_side().unwrap().recipe.exposure, -1.);
        Ok(())
    }

    #[test]
    fn moving_between_photos_keeps_the_reference_on_screen() -> anyhow::Result<()> {
        let mut fixture = fixture()?;
        let (open, other) = (fixture.open, fixture.other);
        let editor = &mut fixture.editor;
        editor.set_reference(other);
        wait_for_reference(editor);
        let shown = super::super::state::Picture::presented(egui::TextureId::Managed(1), [1, 1]);
        editor.preview.before.texture = Some(shown);
        // Left/Right moves the Active photo; the reference stays where it was.
        editor.develop_catalog_photo(other);
        assert_eq!(editor.document.catalog_photo, Some(other));
        assert!(editor.preview.before.texture.is_some());
        assert_eq!(editor.view.compare, Compare::Reference(Axis::LeftRight));
        // Before beside the edit belongs to the photo, and goes with it.
        editor.set_compare(Compare::SideBySide(Axis::LeftRight));
        let shown = super::super::state::Picture::presented(egui::TextureId::Managed(1), [1, 1]);
        editor.preview.before.texture = Some(shown);
        editor.develop_catalog_photo(open);
        assert!(editor.preview.before.texture.is_none());
        Ok(())
    }

    #[test]
    fn leaving_develop_clears_the_reference_unless_it_is_locked() -> anyhow::Result<()> {
        // The fixture keeps its folder for as long as it lives.
        let mut fixture = fixture()?;
        let other = fixture.other;
        let editor = &mut fixture.editor;
        editor.set_reference(other);
        editor.left_develop();
        assert_eq!(editor.view.compare, Compare::Off);
        assert_eq!(editor.reference.photo, None);
        assert!(!editor.reference.loading());
        // Locked, it stays for the next visit to Develop.
        editor.set_reference(other);
        editor.reference.locked = true;
        editor.left_develop();
        assert_eq!(editor.reference.photo, Some(other));
        editor.toggle_reference_view();
        assert_eq!(editor.view.compare, Compare::Reference(Axis::LeftRight));
        // Top/bottom is remembered for Shift+R.
        editor.set_reference_layout(Axis::TopBottom);
        editor.toggle_reference_view();
        assert_eq!(editor.view.compare, Compare::Off);
        editor.toggle_reference_view();
        assert_eq!(editor.view.compare, Compare::Reference(Axis::TopBottom));
        Ok(())
    }

    #[test]
    fn a_photo_develop_cannot_open_is_not_a_reference() -> anyhow::Result<()> {
        // The fixture keeps its folder for as long as it lives.
        let mut fixture = fixture()?;
        let other = fixture.other;
        let editor = &mut fixture.editor;
        let path = editor
            .library
            .as_ref()
            .unwrap()
            .photo(other)
            .unwrap()
            .path
            .clone();
        std::fs::remove_file(path)?;
        editor.set_reference(other);
        assert_eq!(editor.reference.photo, None);
        assert_eq!(editor.view.compare, Compare::Off);
        assert!(editor.status.contains("reference"));
        Ok(())
    }

    #[test]
    fn the_reference_has_its_own_zoom() -> anyhow::Result<()> {
        // The fixture keeps its folder for as long as it lives.
        let mut fixture = fixture()?;
        let other = fixture.other;
        let editor = &mut fixture.editor;
        editor.set_reference(other);
        wait_for_reference(editor);
        editor
            .preview
            .before
            .task
            .finish(editor.preview.before.task.id());
        let fit = editor.preview.before.task.id();
        // The Active photo zoomed in, the reference still fitted.
        editor.view.zoom.on = true;
        editor.schedule_before();
        assert_eq!(editor.preview.before.task.id(), fit);
        // The reference at 100%: a 100% region of it, the Active photo unchanged.
        editor.view.zoom.on = false;
        editor.reference.zoom.on = true;
        editor.schedule_before();
        assert!(editor.preview.before.task.id() > fit);
        assert!(editor.preview.before.zoomed());
        assert!(!editor.view.zoom.on);
        Ok(())
    }

    #[test]
    fn shift_r_shows_reference_view_and_r_still_crops() {
        let ctx = egui::Context::default();
        let mut e = Editor::with_context(&ctx, None, crate::storage::Session::default(), None);
        let press = |modifiers: egui::Modifiers, e: &mut Editor| {
            let key = |pressed| egui::Event::Key {
                key: egui::Key::R,
                physical_key: Some(egui::Key::R),
                pressed,
                repeat: false,
                modifiers,
            };
            let input = egui::RawInput {
                events: vec![
                    egui::Event::ModifiersChanged(modifiers),
                    key(true),
                    key(false),
                ],
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |_| e.develop_shortcuts(&ctx));
            output.textures_delta.clear();
        };
        press(egui::Modifiers::SHIFT, &mut e);
        assert_eq!(e.view.compare, Compare::Reference(Axis::LeftRight));
        assert_eq!(e.view.tool, super::super::state::Tool::None);
        press(egui::Modifiers::SHIFT, &mut e);
        assert_eq!(e.view.compare, Compare::Off);
        press(egui::Modifiers::NONE, &mut e);
        assert_eq!(e.view.tool, super::super::state::Tool::Crop);
    }
}
