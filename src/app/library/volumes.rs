//! Whether each external volume is attached, with its free space, checked off
//! the UI thread so a hung network mount can't stall drawing; and Lightroom's
//! volume header row that shows it.
use crate::app::theme;
use crate::platform::volume::Volume;
use eframe::egui::{self, Color32, Vec2};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

/// Attached, and free/total bytes when known.
pub(super) type VolumeState = (bool, Option<(u64, u64)>);

#[derive(Default)]
pub(super) struct Volumes {
    /// Per volume mount (the startup disk as "/").
    online: Arc<Mutex<HashMap<PathBuf, VolumeState>>>,
    checked: Option<Instant>,
    /// A check is still running, perhaps stuck on a stalled mount: start no other.
    busy: Arc<AtomicBool>,
}
impl Volumes {
    /// Re-checks every few seconds, on a background thread, whether `volumes`
    /// are attached, and asks for a repaint when the answer changes.
    pub(super) fn check<'a>(
        &mut self,
        ctx: &egui::Context,
        volumes: impl Iterator<Item = &'a Volume>,
    ) {
        let due = self
            .checked
            .is_none_or(|t| t.elapsed() > Duration::from_secs(3));
        if !due {
            return;
        }
        self.checked = Some(Instant::now());
        let mounts: Vec<PathBuf> = volumes
            .map(|v| v.mount.clone().unwrap_or_else(|| PathBuf::from("/")))
            .collect();
        ctx.request_repaint_after(Duration::from_secs(3));
        let ctx = ctx.clone();
        spawn_volume_check(
            &self.online,
            &self.busy,
            mounts,
            move || ctx.request_repaint(),
            |m| {
                let attached = m.is_dir();
                let space = attached
                    .then(|| crate::platform::volume::space(m))
                    .flatten();
                (attached, space)
            },
        );
    }
    /// The last check's result for every mount.
    pub(super) fn snapshot(&self) -> HashMap<PathBuf, VolumeState> {
        self.online.lock().unwrap().clone()
    }
}
/// Probes `mounts` on a background thread into `online`, calling `changed` when the
/// result differs, unless the previous check is still running: probing a stalled
/// mount can hang, and new threads would pile up behind it. Returns whether a check
/// started.
pub(super) fn spawn_volume_check(
    online: &Arc<Mutex<HashMap<PathBuf, VolumeState>>>,
    busy: &Arc<AtomicBool>,
    mounts: Vec<PathBuf>,
    changed: impl FnOnce() + Send + 'static,
    probe: impl Fn(&Path) -> VolumeState + Send + 'static,
) -> bool {
    if busy.swap(true, Ordering::Acquire) {
        return false;
    }
    /// Clears the flag however the check ends.
    struct Done(Arc<AtomicBool>);
    impl Drop for Done {
        fn drop(&mut self) {
            self.0.store(false, Ordering::Release);
        }
    }
    let done = Done(busy.clone());
    let online = online.clone();
    std::thread::spawn(move || {
        let _done = done;
        let state: HashMap<PathBuf, VolumeState> = mounts
            .into_iter()
            .map(|m| {
                let state = probe(&m);
                (m, state)
            })
            .collect();
        let mut shared = online.lock().unwrap();
        if *shared != state {
            *shared = state;
            changed();
        }
    });
    true
}
/// A volume's header in the folder tree: a dot lit green when the drive is
/// attached, its name, its free space (or Offline) and a disclosure chevron
/// on the right. Clicking it folds its folders away.
pub(super) fn volume_row(
    ui: &mut egui::Ui,
    volume: &Volume,
    attached: Option<bool>,
    space: Option<(u64, u64)>,
    photos: usize,
    open: bool,
) -> egui::Response {
    let palette = theme::palette(ui.ctx());
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 28.), egui::Sense::click());
    let painter = ui.painter();
    let hovered = response.hovered();
    if hovered {
        painter.rect_filled(rect, 5., palette.gray(32));
    }
    let y = rect.center().y;
    // The dot over the folder tree's chevrons, the name over its folder
    // icons; the chevron ends where the folders' counts do.
    let dot = egui::pos2(rect.left() + 13.5, y);
    if attached == Some(true) {
        painter.circle_filled(dot, 3.5, Color32::from_rgb(110, 200, 90));
    } else {
        let ring = palette.gray(if attached == Some(false) { 150 } else { 90 });
        painter.circle_stroke(dot, 3., egui::Stroke::new(1., ring));
    }
    let chevron = if open {
        crate::app::icons::Icon::ChevronDown
    } else {
        crate::app::icons::Icon::ChevronRight
    };
    crate::app::icons::paint_at(
        painter,
        chevron,
        egui::pos2(rect.right() - 15.5, y),
        11.,
        palette.gray(if hovered { 200 } else { 110 }),
    );
    let gb = |bytes: u64| bytes as f64 / 1e9;
    let detail = match (attached, space) {
        (Some(false), _) => "Offline".to_string(),
        (_, Some((free, _))) => format!("{:.0} GB free", gb(free)),
        _ => String::new(),
    };
    // The free space turns to the warning colour once the drive is over 90% full.
    let full = attached != Some(false)
        && space.is_some_and(|(free, total)| total > 0 && (free as f64) < total as f64 * 0.1);
    let detail_color = if full {
        palette.warning()
    } else {
        palette.gray(125)
    };
    let detail = painter.layout_no_wrap(detail, egui::FontId::proportional(10.5), detail_color);
    let detail_left = rect.right() - 27. - detail.size().x;
    painter.galley(
        egui::pos2(detail_left, y - detail.size().y / 2.),
        detail,
        detail_color,
    );
    let name_left = rect.left() + 22.;
    let name = painter.layout_no_wrap(
        volume.name.clone(),
        egui::FontId::proportional(12.5),
        palette.gray(if hovered { 240 } else { 220 }),
    );
    painter
        .with_clip_rect(rect.with_min_x(name_left).with_max_x(detail_left - 8.))
        .galley(
            egui::pos2(name_left, y - name.size().y / 2.),
            name,
            palette.gray(220),
        );
    let space_text = space
        .map(|(free, total)| format!(" · {:.0} of {:.0} GB free", gb(free), gb(total)))
        .unwrap_or_default();
    response.on_hover_text(match (&volume.mount, attached) {
        (None, _) => format!("Startup disk · {photos} photos{space_text}"),
        (Some(mount), Some(false)) => format!("{} is not attached", mount.display()),
        (Some(mount), _) => format!("{} · {photos} photos{space_text}", mount.display()),
    })
}
