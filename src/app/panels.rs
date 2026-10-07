//! Which of a module's panels show, as Lightroom's Tab, Shift+Tab and F6–F8
//! hide them: each module keeps its own, and the session keeps both across
//! launches. Hiding a panel changes only whether it is drawn, never what it holds.
use super::Editor;
use crate::app::Module;
use eframe::egui::{self, Key, Modifiers};
use serde::{Deserialize, Deserializer, Serialize};

/// A panel the user can hide.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WorkspacePanel {
    /// Develop's Navigator and presets; the Library's Navigator, folders and
    /// collections.
    Left,
    /// Develop's adjustments; the Library's photo info and metadata.
    Right,
    /// The filmstrip with the status bar above it.
    Filmstrip,
}

/// How a request changes a module's panels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PanelChange {
    /// One panel (F7, F8, F6, or its edge arrow).
    Toggle(WorkspacePanel),
    /// Both side panels (Tab).
    Sides,
    /// The side panels and the filmstrip (Shift+Tab).
    All,
}

/// Whether a panel is drawn. Saved by name; any other value reads as shown, so
/// a session from another version never hides a panel by mistake.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", from = "String")]
pub(crate) enum Visibility {
    #[default]
    Shown,
    Hidden,
}
impl From<String> for Visibility {
    fn from(name: String) -> Self {
        if name == "hidden" {
            Self::Hidden
        } else {
            Self::Shown
        }
    }
}
impl Visibility {
    fn flipped(self) -> Self {
        match self {
            Self::Shown => Self::Hidden,
            Self::Hidden => Self::Shown,
        }
    }
}

/// One module's panels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct PanelLayout {
    pub left: Visibility,
    pub right: Visibility,
    pub filmstrip: Visibility,
}
impl PanelLayout {
    pub(super) fn shown(self, panel: WorkspacePanel) -> bool {
        self.visibility(panel) == Visibility::Shown
    }
    fn visibility(self, panel: WorkspacePanel) -> Visibility {
        match panel {
            WorkspacePanel::Left => self.left,
            WorkspacePanel::Right => self.right,
            WorkspacePanel::Filmstrip => self.filmstrip,
        }
    }
    fn slot(&mut self, panel: WorkspacePanel) -> &mut Visibility {
        match panel {
            WorkspacePanel::Left => &mut self.left,
            WorkspacePanel::Right => &mut self.right,
            WorkspacePanel::Filmstrip => &mut self.filmstrip,
        }
    }
    /// The layout after `change`. A group hides when any of it shows, and shows
    /// again only once all of it is hidden, as Lightroom's Tab does.
    pub(super) fn changed(self, change: PanelChange) -> Self {
        let group: &[WorkspacePanel] = match change {
            PanelChange::Toggle(panel) => {
                let mut layout = self;
                *layout.slot(panel) = self.visibility(panel).flipped();
                return layout;
            }
            PanelChange::Sides => &[WorkspacePanel::Left, WorkspacePanel::Right],
            PanelChange::All => &[
                WorkspacePanel::Left,
                WorkspacePanel::Right,
                WorkspacePanel::Filmstrip,
            ],
        };
        let to = if group.iter().any(|panel| self.shown(*panel)) {
            Visibility::Hidden
        } else {
            Visibility::Shown
        };
        let mut layout = self;
        for panel in group {
            *layout.slot(*panel) = to;
        }
        layout
    }
    /// The panels shown here and hidden in `next`.
    pub(super) fn hidden_by(self, next: Self) -> impl Iterator<Item = WorkspacePanel> {
        [
            WorkspacePanel::Left,
            WorkspacePanel::Right,
            WorkspacePanel::Filmstrip,
        ]
        .into_iter()
        .filter(move |panel| self.shown(*panel) && !next.shown(*panel))
    }
}

/// Develop's panels and the Library's, as the session keeps them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct WorkspacePanels {
    pub develop: PanelLayout,
    pub library: PanelLayout,
}
impl WorkspacePanels {
    pub(super) fn of(self, module: Module) -> PanelLayout {
        match module {
            Module::Develop => self.develop,
            Module::Library => self.library,
        }
    }
    pub(super) fn set(&mut self, module: Module, layout: PanelLayout) {
        match module {
            Module::Develop => self.develop = layout,
            Module::Library => self.library = layout,
        }
    }
}

impl Editor {
    /// Whether the module open shows `panel`.
    pub(super) fn panel_shown(&self, panel: WorkspacePanel) -> bool {
        self.panels.of(self.module).shown(panel)
    }
    /// Changes the open module's panels. A panel being hidden first lets go of
    /// what it was doing: the Library's info panel saves a field still being
    /// typed, and stays if that fails (the status line says why); Develop's
    /// presets panel ends a hover preview and keeps a snapshot name being typed.
    /// Returns whether the panels changed.
    pub(super) fn change_panels(&mut self, change: PanelChange) -> bool {
        let layout = self.panels.of(self.module);
        let next = layout.changed(change);
        for panel in layout.hidden_by(next) {
            match (self.module, panel) {
                (Module::Library, WorkspacePanel::Right) => {
                    if !self.commit_library_drafts() {
                        return false;
                    }
                }
                (Module::Develop, WorkspacePanel::Left) => {
                    self.end_preset_hover();
                    self.commit_snapshot_rename();
                }
                _ => {}
            }
        }
        self.panels.set(self.module, next);
        if let Err(e) = self.save_session() {
            self.status = format!("Panel layout not saved: {e:#}");
        }
        true
    }
    /// Tab, Shift+Tab and F6–F8, unless a field or control has the keyboard
    /// focus (Tab then moves it on) or a menu is open. Run before any panel is
    /// drawn, so the Tab taken here does not also focus the first slider.
    pub(super) fn panel_keys(&mut self, ctx: &egui::Context) {
        if ctx.memory(|m| m.focused().is_some()) || egui::Popup::is_any_open(ctx) {
            return;
        }
        let change = ctx.input_mut(|i| {
            // Shift+Tab before Tab: the plain pattern also matches with Shift held.
            if i.consume_key(Modifiers::SHIFT, Key::Tab) {
                Some(PanelChange::All)
            } else if i.consume_key(Modifiers::NONE, Key::Tab) {
                Some(PanelChange::Sides)
            } else if i.consume_key(Modifiers::NONE, Key::F7) {
                Some(PanelChange::Toggle(WorkspacePanel::Left))
            } else if i.consume_key(Modifiers::NONE, Key::F8) {
                Some(PanelChange::Toggle(WorkspacePanel::Right))
            } else if i.consume_key(Modifiers::NONE, Key::F6) {
                Some(PanelChange::Toggle(WorkspacePanel::Filmstrip))
            } else {
                None
            }
        });
        if let Some(change) = change {
            ctx.memory_mut(|m| m.move_focus(egui::FocusDirection::None));
            self.change_panels(change);
        }
    }
}

/// The width of the strip on a window edge that holds a panel's arrow.
const EDGE: f32 = 12.;

impl Editor {
    /// The arrow strips on the left and right window edges, outside the side
    /// panels. Draw after the filmstrip, so they sit above it as the panels do.
    pub(super) fn side_edges(&mut self, ui: &mut egui::Ui) {
        for panel in [WorkspacePanel::Left, WorkspacePanel::Right] {
            self.edge(ui, panel);
        }
    }
    /// The arrow strip on the bottom window edge, under the filmstrip. Draw
    /// before any other bottom panel.
    pub(super) fn bottom_edge(&mut self, ui: &mut egui::Ui) {
        self.edge(ui, WorkspacePanel::Filmstrip);
    }
    /// Lightroom's panel arrow: a thin strip on the window edge beside `panel`,
    /// with a triangle pointing the way the panel would go. A click on the
    /// strip hides or shows the panel.
    fn edge(&mut self, ui: &mut egui::Ui, panel: WorkspacePanel) {
        let visibility = self.panels.of(self.module).visibility(panel);
        let fill = super::theme::palette(ui.ctx()).gray(22);
        let frame = egui::Frame::new().fill(fill);
        let show = |ui: &mut egui::Ui| edge_arrow(ui, panel, visibility);
        let clicked = match panel {
            WorkspacePanel::Left => egui::Panel::left("left-edge")
                .exact_size(EDGE)
                .resizable(false)
                .show_separator_line(false)
                .frame(frame)
                .show(ui, show),
            WorkspacePanel::Right => egui::Panel::right("right-edge")
                .exact_size(EDGE)
                .resizable(false)
                .show_separator_line(false)
                .frame(frame)
                .show(ui, show),
            WorkspacePanel::Filmstrip => egui::Panel::bottom("bottom-edge")
                .exact_size(EDGE)
                .resizable(false)
                .show_separator_line(false)
                .frame(frame)
                .show(ui, show),
        }
        .inner;
        if clicked {
            self.change_panels(PanelChange::Toggle(panel));
        }
    }
}

/// Draws an edge strip's arrow, filling the strip; returns whether it was clicked.
fn edge_arrow(ui: &mut egui::Ui, panel: WorkspacePanel, visibility: Visibility) -> bool {
    let (rect, response) = ui.allocate_exact_size(ui.available_size(), egui::Sense::click());
    let palette = super::theme::palette(ui.ctx());
    let color = if response.hovered() {
        palette.gray(230)
    } else {
        palette.gray(120)
    };
    // Shown, the arrow points off the window, the way the panel goes when hidden.
    let outward = match panel {
        WorkspacePanel::Left => egui::vec2(-1., 0.),
        WorkspacePanel::Right => egui::vec2(1., 0.),
        WorkspacePanel::Filmstrip => egui::vec2(0., 1.),
    };
    let tip = match visibility {
        Visibility::Shown => outward,
        Visibility::Hidden => -outward,
    };
    let across = egui::vec2(tip.y, tip.x);
    let c = rect.center();
    let size = 4.;
    ui.painter().add(egui::Shape::convex_polygon(
        vec![
            c + tip * size,
            c - tip * size + across * size,
            c - tip * size - across * size,
        ],
        color,
        egui::Stroke::NONE,
    ));
    let (name, key) = match panel {
        WorkspacePanel::Left => ("the left panel", "F7"),
        WorkspacePanel::Right => ("the right panel", "F8"),
        WorkspacePanel::Filmstrip => ("the filmstrip", "F6"),
    };
    let verb = match visibility {
        Visibility::Shown => "Hide",
        Visibility::Hidden => "Show",
    };
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(format!("{verb} {name} · {key}"))
        .clicked()
}

/// The session's panels, or all shown if what was saved does not read.
pub(crate) fn lenient<'de, D: Deserializer<'de>>(d: D) -> Result<WorkspacePanels, D::Error> {
    let value = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value(value).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    const HIDDEN: Visibility = Visibility::Hidden;
    const SHOWN: Visibility = Visibility::Shown;

    fn layout(left: Visibility, right: Visibility, filmstrip: Visibility) -> PanelLayout {
        PanelLayout {
            left,
            right,
            filmstrip,
        }
    }

    #[test]
    fn tab_hides_both_sides_while_either_shows_and_brings_both_back() {
        let all = PanelLayout::default();
        let sides_hidden = all.changed(PanelChange::Sides);
        assert_eq!(sides_hidden, layout(HIDDEN, HIDDEN, SHOWN));
        assert_eq!(sides_hidden.changed(PanelChange::Sides), all);
        // One side already hidden: Tab hides the other first.
        let left_only = layout(SHOWN, HIDDEN, SHOWN);
        assert_eq!(left_only.changed(PanelChange::Sides), sides_hidden);
    }

    #[test]
    fn shift_tab_takes_the_filmstrip_with_the_sides() {
        let all = PanelLayout::default();
        let none = all.changed(PanelChange::All);
        assert_eq!(none, layout(HIDDEN, HIDDEN, HIDDEN));
        assert_eq!(none.changed(PanelChange::All), all);
        // Tab's hidden sides with the filmstrip still up: Shift+Tab hides it too.
        assert_eq!(
            layout(HIDDEN, HIDDEN, SHOWN).changed(PanelChange::All),
            none
        );
    }

    #[test]
    fn a_single_toggle_changes_only_its_panel() {
        let all = PanelLayout::default();
        let right = all.changed(PanelChange::Toggle(WorkspacePanel::Right));
        assert_eq!(right, layout(SHOWN, HIDDEN, SHOWN));
        assert_eq!(
            all.hidden_by(right).collect::<Vec<_>>(),
            [WorkspacePanel::Right]
        );
        assert_eq!(
            right.changed(PanelChange::Toggle(WorkspacePanel::Right)),
            all
        );
    }

    #[test]
    fn saved_panels_read_back_and_unknown_values_show() {
        let panels = WorkspacePanels {
            develop: layout(HIDDEN, SHOWN, HIDDEN),
            library: PanelLayout::default(),
        };
        let json = serde_json::to_value(panels).unwrap();
        assert_eq!(json["develop"]["left"], "hidden");
        assert_eq!(
            serde_json::from_value::<WorkspacePanels>(json).unwrap(),
            panels
        );
        let odd: WorkspacePanels = serde_json::from_str(
            r#"{"develop":{"left":"folded","right":"hidden","extra":1},"future":{}}"#,
        )
        .unwrap();
        assert_eq!(odd.develop, layout(SHOWN, HIDDEN, SHOWN));
        assert_eq!(odd.library, PanelLayout::default());
    }
}
