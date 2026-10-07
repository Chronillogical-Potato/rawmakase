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
/// A Lightroom volume header bar: an LED lit green when the drive is
/// attached, the drive name, free / total space (or Offline), and a
/// disclosure arrow that folds its folders away.
pub(super) fn volume_row(
    ui: &mut egui::Ui,
    volume: &Volume,
    attached: Option<bool>,
    space: Option<(u64, u64)>,
    photos: usize,
    open: bool,
) -> egui::Response {
    let palette = theme::palette(ui.ctx());
    ui.add_space(4.);
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 26.), egui::Sense::click());
    let painter = ui.painter();
    painter.rect_filled(
        rect,
        3.,
        palette.gray(if response.hovered() { 64 } else { 56 }),
    );
    let y = rect.center().y;
    let led = egui::Rect::from_center_size(egui::pos2(rect.left() + 14., y), Vec2::new(5., 11.));
    if attached == Some(true) {
        painter.rect_filled(led, 1., Color32::from_rgb(110, 200, 90));
    } else {
        painter.rect_filled(led, 1., palette.gray(26));
        painter.rect_stroke(
            led,
            1.,
            egui::Stroke::new(
                1.,
                palette.gray(if attached == Some(false) { 150 } else { 90 }),
            ),
            egui::StrokeKind::Inside,
        );
    }
    painter.text(
        egui::pos2(rect.left() + 26., y),
        egui::Align2::LEFT_CENTER,
        &volume.name,
        egui::FontId::proportional(12.5),
        palette.gray(225),
    );
    let gb = |bytes: u64| bytes as f64 / 1e9;
    let detail = match (attached, space) {
        (Some(false), _) => "Offline".to_string(),
        (_, Some((free, total))) => format!("{:.0} / {:.0} GB", gb(free), gb(total)),
        _ => String::new(),
    };
    painter.text(
        egui::pos2(rect.right() - 26., y),
        egui::Align2::RIGHT_CENTER,
        detail,
        egui::FontId::proportional(11.),
        palette.gray(160),
    );
    let c = egui::pos2(rect.right() - 13., y);
    let arrow = if open {
        vec![
            c + Vec2::new(-4., -2.),
            c + Vec2::new(4., -2.),
            c + Vec2::new(0., 3.),
        ]
    } else {
        vec![
            c + Vec2::new(3., -4.),
            c + Vec2::new(3., 4.),
            c + Vec2::new(-3., 0.),
        ]
    };
    painter.add(egui::Shape::convex_polygon(
        arrow,
        palette.gray(200),
        egui::Stroke::NONE,
    ));
    response.on_hover_text(match (&volume.mount, attached) {
        (None, _) => format!("Startup disk · {photos} photos"),
        (Some(mount), Some(false)) => format!("{} is not attached", mount.display()),
        (Some(mount), _) => format!("{} · {photos} photos", mount.display()),
    })
}
