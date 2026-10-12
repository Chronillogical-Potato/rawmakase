//! Kinetic scrolling: a trackpad swipe keeps the grid or the filmstrip gliding
//! after the fingers lift, slowing to a stop, as in a browser or Lightroom.
//!
//! The glide starts from the trackpad's own finger-lift event, which Wayland
//! sends and X11 and Windows do not (Windows glides precision touchpads itself),
//! so a mouse wheel never glides. macOS glides itself too, and is left alone.
//! The glide is fed to the scroll area as wheel input, so the area clamps it
//! and stops it at either end; new scrolling, a click, a key, the pointer
//! leaving or the area not being drawn stops it as well.
use eframe::egui::{self, Vec2};

/// Seconds of scrolling before the fingers lift that set the glide's speed.
const SAMPLE_WINDOW: f64 = 0.1;
/// Fingers resting longer than this before they lift stopped the swipe: no glide.
const PAUSE: f64 = 0.06;
/// How quickly a glide slows: its speed falls to about a third in this many seconds.
const TIME_CONSTANT: f32 = 0.35;
/// Glides slower than this, in points a second, do not start, and end when they get here.
const MIN_SPEED: f32 = 50.;
/// The fastest glide, in points a second.
const MAX_SPEED: f32 = 8000.;
/// Whether the platform leaves gliding to the app.
const ENABLED: bool = !cfg!(target_os = "macos");

/// One scroll area's glide, and the swipe that may start the next.
#[derive(Debug, Default)]
pub(super) struct Kinetic {
    /// The swipe so far: each frame's time and the scrolling in it.
    samples: Vec<(f64, Vec2)>,
    /// The area saw the swipe start, with no modifier held then or since:
    /// only such a swipe glides. egui heeds a modifier for the whole of a
    /// swipe, which then zooms or scrolls another way, and a swipe the
    /// pointer brought in from elsewhere may have started with one.
    plain_swipe: bool,
    /// How fast the content glides, in points a second; zero when it does not.
    velocity: Vec2,
    /// When the glide last moved.
    moved_at: f64,
    /// The pass the area was last drawn in.
    drawn_pass: u64,
}

/// What [`Kinetic::begin`] added to the frame's scrolling, for [`Kinetic::end`].
#[must_use]
pub(super) struct Step(Vec2);

impl Kinetic {
    /// Reads this frame's input over the area about to fill the rest of `ui`,
    /// and adds the glide to the scrolling it will take.
    pub(super) fn begin(&mut self, ui: &egui::Ui) -> Step {
        let rect = ui.available_rect_before_wrap();
        let pass = ui.ctx().cumulative_pass_nr();
        let missed = std::mem::replace(&mut self.drawn_pass, pass) + 1 < pass;
        // A pointer leaving the window still counts as over the area for a frame.
        let over = ui.rect_contains_pointer(rect) && ui.input(|i| i.pointer.has_pointer());
        if !ENABLED || missed || !over {
            *self = Self {
                drawn_pass: pass,
                ..Self::default()
            };
            return Step(Vec2::ZERO);
        }
        let delta = ui.input(|i| {
            // The frame's own events: a shortcut taken before the area is
            // drawn still stops the glide.
            let events = &i.raw.events;
            for event in events {
                if let egui::Event::MouseWheel {
                    unit,
                    delta,
                    phase,
                    modifiers,
                } = event
                {
                    self.wheel(i.time, *unit, *delta, *phase, modifiers.is_none());
                }
            }
            // A click or a key in the frame ends the glide and the swipe,
            // whichever came first.
            if i.pointer.any_down()
                || events.iter().any(|e| {
                    matches!(
                        e,
                        egui::Event::PointerButton { pressed: true, .. }
                            | egui::Event::Key { pressed: true, .. }
                    )
                })
            {
                self.stop();
                self.samples.clear();
            }
            self.advance(i.time)
        });
        if delta != Vec2::ZERO {
            ui.ctx().input_mut(|i| i.smooth_scroll_delta += delta);
            ui.ctx().request_repaint();
        }
        Step(delta)
    }
    /// After the area: a direction it could not scroll further ends the glide
    /// there, and its part of the scrolling is taken back.
    pub(super) fn end(&mut self, ui: &egui::Ui, step: Step) {
        let Step(added) = step;
        ui.ctx().input_mut(|i| {
            for d in 0..2 {
                if added[d] != 0. && i.smooth_scroll_delta[d] != 0. {
                    self.velocity[d] = 0.;
                    i.smooth_scroll_delta[d] -= added[d];
                }
            }
        });
    }
    /// Ends the glide, for a scroll the app makes itself.
    pub(super) fn stop(&mut self) {
        self.velocity = Vec2::ZERO;
    }
    /// Takes one wheel event at `time`. Only a trackpad swipe, in points,
    /// with no modifier held (`plain`) can start a glide; any scrolling ends
    /// the one under way.
    fn wheel(
        &mut self,
        time: f64,
        unit: egui::MouseWheelUnit,
        delta: Vec2,
        phase: egui::TouchPhase,
        plain: bool,
    ) {
        self.stop();
        if unit != egui::MouseWheelUnit::Point {
            self.samples.clear();
            return;
        }
        match phase {
            egui::TouchPhase::Start => {
                self.samples.clear();
                self.plain_swipe = plain;
            }
            egui::TouchPhase::Cancel => self.samples.clear(),
            egui::TouchPhase::Move if !self.plain_swipe || !plain => {
                self.plain_swipe = false;
                self.samples.clear();
            }
            egui::TouchPhase::Move => {
                self.samples.retain(|(at, _)| *at >= time - SAMPLE_WINDOW);
                match self.samples.last_mut() {
                    Some((at, sum)) if *at == time => *sum += delta,
                    _ => self.samples.push((time, delta)),
                }
            }
            egui::TouchPhase::End => {
                if std::mem::take(&mut self.plain_swipe) {
                    self.velocity = release_velocity(&self.samples, time);
                }
                self.moved_at = time;
                self.samples.clear();
            }
        }
    }
    /// How far the glide moves the content by `time`, slowing it down.
    fn advance(&mut self, time: f64) -> Vec2 {
        if self.velocity == Vec2::ZERO {
            return Vec2::ZERO;
        }
        let dt = (time - std::mem::replace(&mut self.moved_at, time)).max(0.) as f32;
        let decay = (-dt / TIME_CONSTANT).exp();
        let moved = self.velocity * TIME_CONSTANT * (1. - decay);
        self.velocity *= decay;
        if self.velocity.length() < MIN_SPEED {
            self.stop();
        }
        moved
    }
}

/// The speed of a swipe whose fingers lifted at `time`: the scrolling in its
/// last moments over their duration, or zero when it had already stopped.
fn release_velocity(samples: &[(f64, Vec2)], time: f64) -> Vec2 {
    let recent: Vec<_> = samples
        .iter()
        .filter(|(at, _)| *at >= time - SAMPLE_WINDOW)
        .collect();
    let (Some((first, _)), Some((last, _))) = (recent.first(), recent.last()) else {
        return Vec2::ZERO;
    };
    if time - last > PAUSE || last <= first {
        return Vec2::ZERO;
    }
    let moved = recent[1..].iter().fold(Vec2::ZERO, |sum, (_, d)| sum + *d);
    let velocity = moved / (last - first) as f32;
    let speed = velocity.length();
    if speed < MIN_SPEED {
        Vec2::ZERO
    } else {
        velocity * (MAX_SPEED / speed).min(1.)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{MouseWheelUnit, TouchPhase};

    const FRAME: f64 = 1. / 60.;

    /// Starts a swipe down at `start` and moves `frames` frames of `step`
    /// points each, `plain` or with a modifier held; returns the last move's time.
    fn swipe_samples(
        kinetic: &mut Kinetic,
        start: f64,
        frames: usize,
        step: f32,
        plain: bool,
    ) -> f64 {
        kinetic.wheel(
            start,
            MouseWheelUnit::Point,
            Vec2::ZERO,
            TouchPhase::Start,
            plain,
        );
        let mut t = start;
        for _ in 0..frames {
            t += FRAME;
            kinetic.wheel(
                t,
                MouseWheelUnit::Point,
                Vec2::new(0., step),
                TouchPhase::Move,
                plain,
            );
        }
        t
    }

    /// Swipes down in `frames` frames of `step` points each from `start`,
    /// then lifts the fingers a frame later; returns when they lifted.
    fn swipe(kinetic: &mut Kinetic, start: f64, frames: usize, step: f32) -> f64 {
        let t = swipe_samples(kinetic, start, frames, step, true) + FRAME;
        kinetic.wheel(t, MouseWheelUnit::Point, Vec2::ZERO, TouchPhase::End, true);
        t
    }

    #[test]
    fn a_swipe_glides_on_in_its_direction_and_comes_to_rest() {
        let mut kinetic = Kinetic::default();
        let mut t = swipe(&mut kinetic, 0., 6, -20.);
        let mut steps = Vec::new();
        while kinetic.velocity != Vec2::ZERO {
            t += FRAME;
            steps.push(kinetic.advance(t).y);
            assert!(t < 3., "the glide never stopped");
        }
        assert!(steps.iter().all(|s| *s < 0.), "{steps:?}");
        assert!(steps.windows(2).all(|w| w[1] > w[0]), "it slows down");
        let total: f32 = steps.iter().sum();
        // 1200 points a second, gliding about TIME_CONSTANT seconds' worth.
        assert!((-1200. * TIME_CONSTANT..-300.).contains(&total), "{total}");
        assert!((0.5..2.5).contains(&t), "glided for {t} s");
    }

    #[test]
    fn a_mouse_wheel_does_not_glide() {
        let mut kinetic = Kinetic::default();
        for n in 0..5 {
            let t = n as f64 * FRAME;
            kinetic.wheel(
                t,
                MouseWheelUnit::Line,
                Vec2::new(0., -1.),
                TouchPhase::Move,
                true,
            );
        }
        kinetic.wheel(0.1, MouseWheelUnit::Line, Vec2::ZERO, TouchPhase::End, true);
        assert_eq!(kinetic.velocity, Vec2::ZERO);
    }

    #[test]
    fn fingers_resting_before_they_lift_do_not_glide() {
        // Lifted `pause` after the last move: within the sampling window
        // either way, but past the pause only the second time.
        let lift = |pause: f64| {
            let mut kinetic = Kinetic::default();
            let last = swipe_samples(&mut kinetic, 0., 6, -20., true);
            let end = last + pause;
            kinetic.wheel(
                end,
                MouseWheelUnit::Point,
                Vec2::ZERO,
                TouchPhase::End,
                true,
            );
            kinetic.velocity
        };
        assert_ne!(lift(0.04), Vec2::ZERO);
        assert_eq!(lift(0.08), Vec2::ZERO);
    }

    #[test]
    fn a_swipe_with_a_modifier_does_not_glide() {
        for modifiers in [
            egui::Modifiers::COMMAND,
            egui::Modifiers::ALT,
            egui::Modifiers::SHIFT,
        ] {
            let mut kinetic = Kinetic::default();
            let t = swipe_samples(&mut kinetic, 0., 6, -20., modifiers.is_none());
            kinetic.wheel(
                t + FRAME,
                MouseWheelUnit::Point,
                Vec2::ZERO,
                TouchPhase::End,
                true,
            );
            assert_eq!(kinetic.velocity, Vec2::ZERO, "{modifiers:?}");
        }
    }

    #[test]
    fn a_modifier_let_go_of_mid_swipe_still_does_not_glide() {
        // egui keeps a modifier for the rest of the swipe, so it zoomed or
        // scrolled sideways throughout.
        let mut kinetic = Kinetic::default();
        let t = swipe_samples(&mut kinetic, 0., 2, -20., false);
        let mut t2 = t;
        for _ in 0..6 {
            t2 += FRAME;
            kinetic.wheel(
                t2,
                MouseWheelUnit::Point,
                Vec2::new(0., -20.),
                TouchPhase::Move,
                true,
            );
        }
        kinetic.wheel(
            t2 + FRAME,
            MouseWheelUnit::Point,
            Vec2::ZERO,
            TouchPhase::End,
            true,
        );
        assert_eq!(kinetic.velocity, Vec2::ZERO);
        // The next swipe without it glides again.
        swipe(&mut kinetic, 1., 6, -20.);
        assert_ne!(kinetic.velocity, Vec2::ZERO);
    }

    #[test]
    fn a_swipe_whose_start_the_area_missed_does_not_glide() {
        // The pointer came in mid-swipe: the swipe may have started with a
        // modifier egui still heeds.
        let mut kinetic = Kinetic::default();
        let mut t = 0.;
        for _ in 0..6 {
            t += FRAME;
            kinetic.wheel(
                t,
                MouseWheelUnit::Point,
                Vec2::new(0., -20.),
                TouchPhase::Move,
                true,
            );
        }
        kinetic.wheel(
            t + FRAME,
            MouseWheelUnit::Point,
            Vec2::ZERO,
            TouchPhase::End,
            true,
        );
        assert_eq!(kinetic.velocity, Vec2::ZERO);
    }

    #[test]
    fn new_scrolling_stops_a_glide() {
        let mut kinetic = Kinetic::default();
        let t = swipe(&mut kinetic, 0., 6, -20.);
        kinetic.wheel(
            t + FRAME,
            MouseWheelUnit::Point,
            Vec2::ZERO,
            TouchPhase::Start,
            true,
        );
        assert_eq!(kinetic.advance(t + 2. * FRAME), Vec2::ZERO);
    }

    /// The glide through a real scroll area, from the trackpad's events.
    struct Area {
        ctx: egui::Context,
        kinetic: Kinetic,
        time: f64,
        offset: f32,
        /// How far the content can scroll.
        end: f32,
    }
    impl Area {
        fn new() -> Self {
            let mut area = Self {
                ctx: egui::Context::default(),
                kinetic: Kinetic::default(),
                time: 0.,
                offset: 0.,
                end: 0.,
            };
            area.frame(vec![egui::Event::PointerMoved(egui::pos2(100., 100.))]);
            area
        }
        /// Content `rows` of 100 points in a 400-point-tall area.
        fn frame_with(&mut self, events: Vec<egui::Event>, rows: usize) {
            self.time += FRAME;
            let (kinetic, offset, end) = (&mut self.kinetic, &mut self.offset, &mut self.end);
            let mut output = self.ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        Vec2::new(400., 400.),
                    )),
                    time: Some(self.time),
                    events,
                    ..Default::default()
                },
                |ui| {
                    // A shortcut taken before the area is drawn, as Tab is.
                    ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Tab));
                    let step = kinetic.begin(ui);
                    let output = egui::ScrollArea::vertical().auto_shrink(false).show_rows(
                        ui,
                        100.,
                        rows,
                        |ui, rows| {
                            for row in rows {
                                ui.allocate_space(Vec2::new(10., 100.));
                                let _ = row;
                            }
                        },
                    );
                    kinetic.end(ui, step);
                    *offset = output.state.offset.y;
                    *end = output.content_size.y - output.inner_rect.height();
                },
            );
            output.textures_delta.clear();
        }
        fn frame(&mut self, events: Vec<egui::Event>) {
            self.frame_with(events, 100);
        }
        fn wheel(phase: TouchPhase, dy: f32) -> egui::Event {
            egui::Event::MouseWheel {
                unit: MouseWheelUnit::Point,
                delta: Vec2::new(0., dy),
                phase,
                modifiers: egui::Modifiers::NONE,
            }
        }
        fn swipe(&mut self, rows: usize) {
            self.swipe_lifted_with(rows, vec![Self::wheel(TouchPhase::End, 0.)]);
        }
        /// A swipe whose last frame has `lift`: the fingers lifting and
        /// whatever else came with them.
        fn swipe_lifted_with(&mut self, rows: usize, lift: Vec<egui::Event>) {
            self.frame_with(vec![Self::wheel(TouchPhase::Start, 0.)], rows);
            for _ in 0..6 {
                self.frame_with(vec![Self::wheel(TouchPhase::Move, -20.)], rows);
            }
            self.frame_with(lift, rows);
        }
        fn key(key: egui::Key) -> egui::Event {
            egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }
        }
    }

    #[test]
    fn the_area_keeps_scrolling_after_the_fingers_lift() {
        if !ENABLED {
            return;
        }
        let mut area = Area::new();
        area.swipe(100);
        let lifted = area.offset;
        assert!(lifted > 100., "the swipe itself scrolled {lifted}");
        for _ in 0..10 {
            area.frame(Vec::new());
        }
        assert!(
            area.offset > lifted + 100.,
            "{} after {lifted}",
            area.offset
        );
    }

    #[test]
    fn the_glide_ends_at_the_bottom() {
        if !ENABLED {
            return;
        }
        let mut area = Area::new();
        // Six rows in a four-row area: the glide could carry the content
        // further than its end.
        area.swipe(6);
        for _ in 0..10 {
            area.frame_with(Vec::new(), 6);
            if area.kinetic.velocity == Vec2::ZERO {
                break;
            }
        }
        assert_eq!(area.kinetic.velocity, Vec2::ZERO);
        assert_eq!(area.offset, area.end);
    }

    #[test]
    fn a_click_stops_the_glide() {
        if !ENABLED {
            return;
        }
        let mut area = Area::new();
        area.swipe(100);
        let at = egui::pos2(100., 100.);
        area.frame(vec![egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        }]);
        let stopped = area.offset;
        area.frame(vec![egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }]);
        area.frame(Vec::new());
        assert_eq!(area.offset, stopped);
    }

    #[test]
    fn a_shortcut_taken_before_the_area_stops_the_glide() {
        if !ENABLED {
            return;
        }
        let mut area = Area::new();
        area.swipe(100);
        area.frame(Vec::new());
        assert_ne!(area.kinetic.velocity, Vec2::ZERO);
        area.frame(vec![Area::key(egui::Key::Tab)]);
        let stopped = area.offset;
        area.frame(Vec::new());
        assert_eq!(area.offset, stopped);
    }

    #[test]
    fn a_key_with_the_fingers_lifting_does_not_glide() {
        if !ENABLED {
            return;
        }
        let lift = Area::wheel(TouchPhase::End, 0.);
        let key = Area::key(egui::Key::ArrowDown);
        for events in [vec![lift.clone(), key.clone()], vec![key, lift]] {
            let mut area = Area::new();
            area.swipe_lifted_with(100, events);
            let lifted = area.offset;
            area.frame(Vec::new());
            assert_eq!(area.offset, lifted);
        }
    }

    #[test]
    fn a_swipe_brought_into_the_area_does_not_glide() {
        if !ENABLED {
            return;
        }
        let mut area = Area::new();
        // The swipe starts with Ctrl held while the pointer is elsewhere.
        area.frame(vec![
            egui::Event::PointerGone,
            egui::Event::MouseWheel {
                unit: MouseWheelUnit::Point,
                delta: Vec2::ZERO,
                phase: TouchPhase::Start,
                modifiers: egui::Modifiers::CTRL,
            },
        ]);
        area.frame(vec![egui::Event::PointerMoved(egui::pos2(100., 100.))]);
        for _ in 0..6 {
            area.frame(vec![Area::wheel(TouchPhase::Move, -20.)]);
        }
        area.frame(vec![Area::wheel(TouchPhase::End, 0.)]);
        assert_eq!(area.kinetic.velocity, Vec2::ZERO);
    }

    #[test]
    fn leaving_the_area_stops_the_glide() {
        if !ENABLED {
            return;
        }
        let mut area = Area::new();
        area.swipe(100);
        area.frame(vec![egui::Event::PointerGone]);
        let left = area.offset;
        area.frame(vec![egui::Event::PointerMoved(egui::pos2(100., 100.))]);
        area.frame(Vec::new());
        assert_eq!(area.offset, left);
    }
}
