//! Interface colours. RAWmakase's own palette is Lightroom's neutral greys;
//! fastframe-theme adds palette files, its presets and, on Linux, the
//! Omarchy desktop's theme, followed live.
//!
//! The interface was drawn in greys, so a palette is applied through them:
//! [`gray`] maps a level of the default palette onto the current one,
//! between the palette's own colours. The photo's backdrop is a neutral
//! grey in every theme, as light as the palette, so colours are judged
//! against grey.
use eframe::egui::{self, Color32, Stroke};
use std::collections::BTreeSet;
use std::sync::RwLock;

/// fastframe-theme's sixteen interface colours.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Palette {
    light: bool,
    window: Color32,
    panel: Color32,
    surface: Color32,
    surface_hover: Color32,
    surface_active: Color32,
    outline: Color32,
    text: Color32,
    secondary: Color32,
    dim: Color32,
    accent: Color32,
    accent_hover: Color32,
    on_accent: Color32,
    danger: Color32,
    warning: Color32,
    overlay: Color32,
    shadow: Color32,
}

/// The grey each colour stands for in the default palette, darkest first.
/// Lightroom's panels, controls and text sit at these levels.
const LEVELS: [u8; 9] = [22, 35, 43, 52, 59, 67, 125, 160, 225];

impl Palette {
    /// Lightroom's dark greys, which the interface was drawn in.
    pub(super) const DEFAULT: Palette = Palette {
        light: false,
        window: Color32::from_gray(22),
        panel: Color32::from_gray(35),
        surface: Color32::from_gray(43),
        surface_hover: Color32::from_gray(59),
        surface_active: Color32::from_gray(67),
        outline: Color32::from_gray(52),
        text: Color32::from_gray(225),
        secondary: Color32::from_gray(160),
        dim: Color32::from_gray(125),
        accent: Color32::from_rgb(62, 88, 115),
        accent_hover: Color32::from_rgb(74, 104, 136),
        on_accent: Color32::WHITE,
        danger: Color32::from_rgb(214, 120, 110),
        warning: Color32::from_rgb(222, 184, 92),
        overlay: Color32::from_gray(33),
        shadow: Color32::from_black_alpha(110),
    };
    /// Lightroom's lightest interface greys, for light palette files.
    const LIGHT: Palette = Palette {
        light: true,
        window: Color32::from_gray(236),
        panel: Color32::from_gray(222),
        surface: Color32::from_gray(210),
        surface_hover: Color32::from_gray(198),
        surface_active: Color32::from_gray(188),
        outline: Color32::from_gray(180),
        text: Color32::from_gray(28),
        secondary: Color32::from_gray(80),
        dim: Color32::from_gray(120),
        accent: Color32::from_rgb(52, 94, 140),
        accent_hover: Color32::from_rgb(64, 108, 156),
        on_accent: Color32::WHITE,
        danger: Color32::from_rgb(180, 60, 60),
        warning: Color32::from_rgb(170, 120, 20),
        overlay: Color32::from_gray(228),
        shadow: Color32::from_black_alpha(60),
    };
    /// The palette's colour for each of [`LEVELS`].
    fn anchors(&self) -> [Color32; 9] {
        [
            self.window,
            self.panel,
            self.surface,
            self.outline,
            self.surface_hover,
            self.surface_active,
            self.dim,
            self.secondary,
            self.text,
        ]
    }
}

impl fastframe_theme::Palette for Palette {
    fn base(base: fastframe_theme::Base) -> Self {
        match base {
            fastframe_theme::Base::Dark => Self::DEFAULT,
            fastframe_theme::Base::Light => Self::LIGHT,
        }
    }
    fn set(&mut self, name: &str, color: Color32) -> bool {
        let slot = match name {
            "window" => &mut self.window,
            "panel" => &mut self.panel,
            "surface" => &mut self.surface,
            "surface_hover" => &mut self.surface_hover,
            "surface_active" => &mut self.surface_active,
            "outline" => &mut self.outline,
            "text" => &mut self.text,
            "secondary" => &mut self.secondary,
            "dim" => &mut self.dim,
            "accent" => &mut self.accent,
            "accent_hover" => &mut self.accent_hover,
            "on_accent" => &mut self.on_accent,
            "danger" => &mut self.danger,
            "warning" => &mut self.warning,
            "overlay" => &mut self.overlay,
            "shadow" => &mut self.shadow,
            _ => return false,
        };
        *slot = color;
        true
    }
    fn derive(&mut self, given: &BTreeSet<&str>) {
        // A light window under dark text, whatever base the file named.
        self.light = luma(self.window) > luma(self.text);
        // Menus and pop-ups sit on the panel colour unless a file says.
        if !given.contains("overlay") && given.contains("panel") {
            self.overlay = self.panel;
        }
    }
}

static CURRENT: RwLock<Palette> = RwLock::new(Palette::DEFAULT);

pub(super) fn current() -> Palette {
    *CURRENT.read().unwrap_or_else(|e| e.into_inner())
}

/// The default palette's grey `level` in the current palette: between the
/// two palette colours whose levels surround it, and beyond the darkest and
/// lightest along the same direction.
pub(super) fn gray(level: u8) -> Color32 {
    let palette = current();
    if palette == Palette::DEFAULT {
        return Color32::from_gray(level);
    }
    map(&palette, level)
}

fn map(palette: &Palette, level: u8) -> Color32 {
    // Dark text on a light window reads fainter than light text on a dark
    // one at the same mix, so on light palettes text levels lean towards
    // the text colour rather than following the palette's dim and secondary.
    let (text_from, text_to) = (LEVELS[5], LEVELS[8]);
    if palette.light && (text_from..=text_to).contains(&level) {
        let f = (level - text_from) as f32 / (text_to - text_from) as f32;
        // The palette's text is often a soft slate; the brightest levels
        // (headings, values) get a darker ink so they stand out on it.
        let ink = lerp(palette.text, Color32::BLACK, 0.3);
        return lerp(palette.surface_active, ink, 1. - (1. - f).powi(3));
    }
    let anchors = palette.anchors();
    let i = LEVELS
        .windows(2)
        .position(|w| level <= w[1])
        .unwrap_or(LEVELS.len() - 2);
    let (a, b) = (LEVELS[i] as f32, LEVELS[i + 1] as f32);
    let t = (level as f32 - a) / (b - a);
    lerp(anchors[i], anchors[i + 1], t)
}

/// Linear between `a` (t = 0) and `b` (t = 1), extrapolating outside that.
fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let channel = |x: u8, y: u8| {
        (x as f32 + (y as f32 - x as f32) * t)
            .round()
            .clamp(0., 255.) as u8
    };
    Color32::from_rgb(
        channel(a.r(), b.r()),
        channel(a.g(), b.g()),
        channel(a.b(), b.b()),
    )
}

/// Behind the photo and the Navigator: a neutral grey as light as the
/// palette's window colour, so photos are judged against grey in any theme
/// (Lightroom's own backdrops are greys too).
pub(super) fn photo_backdrop() -> Color32 {
    Color32::from_gray(luma(current().window).round() as u8)
}
fn luma(c: Color32) -> f32 {
    0.2126 * c.r() as f32 + 0.7152 * c.g() as f32 + 0.0722 * c.b() as f32
}

/// A selected row in the folder tree and preset list: Lightroom's slate
/// blue-grey, or the palette's surface tinted towards its accent so the
/// row's text stays readable.
pub(super) fn selected_row() -> Color32 {
    let palette = current();
    if palette == Palette::DEFAULT {
        return Color32::from_rgb(47, 58, 66);
    }
    lerp(palette.surface_active, palette.accent, 0.3)
}
/// The bar beside a selected folder.
pub(super) fn selected_marker() -> Color32 {
    let palette = current();
    if palette == Palette::DEFAULT {
        return Color32::from_rgb(135, 160, 176);
    }
    palette.accent
}

/// The selection and primary-button colour.
pub(super) fn accent() -> Color32 {
    current().accent
}
/// Text on a selected row or hovered menu item, which the accent fills.
pub(super) fn on_accent_text(level: u8) -> Color32 {
    let palette = current();
    if palette == Palette::DEFAULT {
        return Color32::from_gray(level);
    }
    palette.on_accent
}
/// Text on the accent colour.
pub(super) fn on_accent() -> Color32 {
    current().on_accent
}
pub(super) fn danger() -> Color32 {
    current().danger
}

/// Makes `palette` current and styles egui's own widgets with it.
pub(super) fn apply(ctx: &egui::Context, palette: Palette, text: &fastframe_text::TextRendering) {
    *CURRENT.write().unwrap_or_else(|e| e.into_inner()) = palette;
    let mut visuals = if palette.light {
        egui::Visuals::light()
    } else {
        egui::Visuals::dark()
    };
    visuals.panel_fill = gray(35);
    visuals.window_fill = gray(35);
    visuals.extreme_bg_color = gray(22);
    visuals.faint_bg_color = gray(40);
    visuals.code_bg_color = gray(43);
    visuals.selection.bg_fill = palette.accent;
    visuals.selection.stroke = Stroke::new(1., palette.on_accent);
    visuals.hyperlink_color = palette.accent_hover;
    visuals.error_fg_color = palette.danger;
    visuals.warn_fg_color = palette.warning;
    visuals.window_stroke = Stroke::new(1., palette.outline);
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1., palette.outline);
    visuals.widgets.noninteractive.bg_fill = gray(35);
    visuals.widgets.noninteractive.weak_bg_fill = gray(35);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1., gray(194));
    visuals.widgets.inactive.bg_fill = gray(43);
    visuals.widgets.inactive.weak_bg_fill = gray(43);
    visuals.widgets.hovered.bg_fill = gray(59);
    visuals.widgets.hovered.weak_bg_fill = gray(59);
    visuals.widgets.active.bg_fill = gray(67);
    visuals.widgets.active.weak_bg_fill = gray(67);
    visuals.widgets.open.bg_fill = gray(59);
    visuals.widgets.open.weak_bg_fill = gray(59);
    if palette != Palette::DEFAULT {
        visuals.widgets.inactive.fg_stroke = Stroke::new(1., gray(205));
        visuals.widgets.hovered.fg_stroke = Stroke::new(1.5, gray(240));
        visuals.widgets.active.fg_stroke = Stroke::new(2., gray(250));
        visuals.widgets.open.fg_stroke = Stroke::new(1., gray(225));
    }
    // egui insets button text by the stroke width, but an unframed item
    // (a menu or list row) has no stroke until hovered, so its text moved
    // by a pixel. No widget strokes: fills alone show state.
    for widget in [
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widget.bg_stroke = Stroke::NONE;
    }
    // egui grows hovered widgets by a pixel; keep every control a fixed size.
    for widget in [
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widget.expansion = 0.;
    }
    text.apply_to_visuals(&mut visuals);
    // egui draws glyphs thinner in light mode, which leaves a light palette's
    // text faint; give it the dark mode's heavier coverage. Linux keeps its
    // measured linear coverage.
    if palette.light
        && visuals.text_options.color_transfer_function
            == egui::epaint::FontColorTransferFunction::LIGHT_MODE_DEFAULT
    {
        visuals.text_options.color_transfer_function =
            egui::epaint::FontColorTransferFunction::DARK_MODE_DEFAULT;
    }
    // The interface has one appearance whatever the system's light or dark
    // mode, as Lightroom does.
    ctx.set_theme(if palette.light {
        egui::Theme::Light
    } else {
        egui::Theme::Dark
    });
    ctx.set_visuals(visuals);
}

/// The palettes the Display preferences offer, and the one chosen.
pub(super) struct Themes {
    pub(super) catalog: fastframe_theme::Catalog<Palette>,
    /// A palette file's name; None is RAWmakase's own greys.
    pub(super) selected: Option<String>,
    applied: Palette,
    pub(super) text: fastframe_text::TextRendering,
    waker: fastframe_theme::Waker,
    started: bool,
}
impl Themes {
    pub(super) fn new(
        ctx: &egui::Context,
        selected: Option<String>,
        text: fastframe_text::TextRendering,
    ) -> Self {
        let wake = ctx.clone();
        Self {
            catalog: Default::default(),
            selected,
            applied: Palette::DEFAULT,
            text,
            waker: fastframe_theme::Waker::new(move || wake.request_repaint()),
            started: false,
        }
    }
    fn directory() -> std::path::PathBuf {
        crate::storage::data_dir().join("themes")
    }
    /// Lists the palettes in the background: the presets, the user's files
    /// and, on Linux, Omarchy's current theme.
    pub(super) fn start(&mut self) {
        self.catalog
            .enable_desktop_themes(fastframe_theme::DesktopThemes {
                slug: "rawmakase",
                omarchy_template: fastframe_theme::omarchy::BASE_TEMPLATE,
                omarchy_previous_templates: &[],
                presets: true,
            });
        self.catalog
            .start(Self::directory(), self.selected.clone(), &self.waker);
        self.started = true;
    }
    /// Rescans on a change to the themes folder or Omarchy's theme, and
    /// applies the chosen palette once it is read or when it changes.
    pub(super) fn poll(&mut self, ctx: &egui::Context) {
        if !self.started {
            return;
        }
        if self.catalog.needs_reload() {
            self.catalog
                .start(Self::directory(), self.selected.clone(), &self.waker);
        }
        self.catalog.poll();
        let wanted = self.resolve();
        if wanted != self.applied {
            self.applied = wanted;
            apply(ctx, wanted, &self.text);
        }
    }
    /// The chosen palette, or the last one applied while it is being read.
    fn resolve(&self) -> Palette {
        match &self.selected {
            None => Palette::DEFAULT,
            Some(name) => self
                .catalog
                .find(name)
                .map_or(self.applied, |theme| theme.palette),
        }
    }
    /// Choices for the theme menu: file name and display name.
    pub(super) fn choices(&self) -> Vec<(String, String)> {
        self.catalog
            .picker_themes()
            .map(|t| {
                let name = fastframe_theme::display_name(&t.filename).to_string();
                (t.filename.clone(), name)
            })
            .collect()
    }
    /// What to say under the setting, if anything.
    pub(super) fn status(&self) -> Option<String> {
        use fastframe_theme::{Problem, Status};
        Some(match self.catalog.status(self.selected.as_deref())? {
            Status::Loading => "Loading themes…".into(),
            Status::SelectedUnavailable => {
                "The chosen theme is missing; the last one stays until it is back.".into()
            }
            Status::Problem(problem) => match problem {
                Problem::TooManyEntries | Problem::TooManyThemes => {
                    "The themes folder has too many files; some are not listed.".into()
                }
                Problem::OmarchyUnreadable => "Omarchy’s current theme could not be read.".into(),
                _ => "The themes folder could not be read.".into(),
            },
        })
    }
    pub(super) fn folder() -> std::path::PathBuf {
        Self::directory()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_palette_maps_every_grey_to_itself() {
        for level in 0..=255u8 {
            assert_eq!(map(&Palette::DEFAULT, level), Color32::from_gray(level));
        }
    }

    #[test]
    fn a_palette_replaces_the_greys_at_its_levels() {
        let nord: Palette = fastframe_theme::parse_palette(
            r##"{"colors":{"window":"#2e3440","text":"#d8dee9","accent":"#81a1c1"}}"##,
        )
        .unwrap();
        assert_eq!(map(&nord, 22), Color32::from_rgb(0x2e, 0x34, 0x40));
        assert_eq!(map(&nord, 225), Color32::from_rgb(0xd8, 0xde, 0xe9));
        assert_eq!(nord.accent, Color32::from_rgb(0x81, 0xa1, 0xc1));
    }
}
