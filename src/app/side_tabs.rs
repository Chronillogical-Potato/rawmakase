//! Develop's tab panel beside the adjustments: Presets, and Snapshots over
//! History, one at a time, or the open tool's drawer. A slim rail at the
//! window's right edge switches the tab and, under them, opens Crop, Remove,
//! Red Eye or Masking; the open tab's button folds the panel and any brings
//! it back. A window too narrow for it and the photo shows it folded until a
//! tab or tool is asked for.
use super::Editor;
use super::icons::{self, Icon};
use super::inspector::{DRAWER_TOOLS, has_drawer, paint_tool_icon};
use super::panels::{DevelopTab, PanelChange, WorkspacePanel};
use super::state::Tool;
use super::theme;
use super::widgets::{SectionGroup, SectionSide};
use eframe::egui::{self, Pos2, Rect, Sense, Vec2};

/// The rail's width.
const RAIL_WIDTH: f32 = 36.;
/// The tab panel's width until the user drags it.
const PANEL_WIDTH: f32 = 245.;
/// The slider grid's label column in the tool drawers, wide enough for
/// their longest captions ("Orientation", "Range High").
const DRAWER_LABEL_COLUMN: f32 = 68.;
/// The photo's narrowest width before the tab panel folds by itself.
const PHOTO_MIN: f32 = 400.;
/// The tabs, top to bottom on the rail, with their icons and names.
const TABS: [(DevelopTab, Icon, &str); 2] = [
    (DevelopTab::Presets, Icon::Presets, "Presets"),
    (DevelopTab::History, Icon::History, "History"),
];

/// What a click on the rail asked for.
enum TabClick {
    /// Show this tab, opening the panel.
    Open(DevelopTab),
    Fold,
    /// Open or close this tool, its drawer in the panel.
    Tool(Tool),
}

/// A rail button's id, fixed so tests can find it.
pub(super) fn rail_id(tab: DevelopTab) -> egui::Id {
    egui::Id::new(("develop-rail", tab))
}
/// Whether a tab was asked for while the window had no room for the panel.
fn peek_id() -> egui::Id {
    egui::Id::new("develop-tabs-peek")
}
/// Whether the window had room for the panel beside the photo last frame.
fn room_id() -> egui::Id {
    egui::Id::new("develop-tabs-room")
}

impl Editor {
    /// The rail, drawn first so it takes the window's right edge, outside
    /// the adjustments. The right panel's toggle (F8) hides it, the tab panel
    /// and the adjustments together; the rail folds the tab panel alone.
    /// Returns whether the tab panel opens, for [`Self::develop_tab_panel`]
    /// to draw once the adjustments are.
    pub(super) fn develop_rail_edge(&mut self, ui: &mut egui::Ui) -> bool {
        let ctx = ui.ctx().clone();
        if !self.panel_shown(WorkspacePanel::Right) {
            return false;
        }
        let width = |id: &str, default: f32| {
            egui::containers::panel::PanelState::load(&ctx, egui::Id::new(id))
                .map_or(default, |state| state.outer_rect.width())
        };
        let adjustments = width("adjustments", super::workspace::ADJUSTMENTS_DEFAULT);
        let room = ui.available_width() - RAIL_WIDTH - adjustments - width("presets", PANEL_WIDTH)
            >= PHOTO_MIN;
        ctx.data_mut(|d| d.insert_temp(room_id(), room));
        let click = self.develop_rail(ui, self.develop_tabs_open());
        match click {
            Some(TabClick::Open(tab)) => {
                self.set_develop_tab(tab);
                self.open_develop_tabs();
            }
            Some(TabClick::Fold) => {
                self.fold_develop_tabs();
            }
            Some(TabClick::Tool(tool)) => self.view.toggle(tool),
            None => {}
        }
        // A tool's drawer opens the panel whatever its fold or the room.
        has_drawer(self.view.tool) || self.develop_tabs_open()
    }
    /// Whether the tab panel shows its tab: not folded, and with room beside
    /// the photo as last laid out, or asked for in a narrow window.
    fn develop_tabs_open(&self) -> bool {
        let (room, peek) = self.context.data(|d| {
            (
                d.get_temp::<bool>(room_id()).unwrap_or(true),
                d.get_temp::<bool>(peek_id()).unwrap_or(false),
            )
        });
        self.panel_shown(WorkspacePanel::Presets) && (room || peek)
    }
    /// Opens the tab panel on its tab, in a narrow window too, ending a
    /// tool whose drawer had its place; false if the layout could not be
    /// saved.
    fn open_develop_tabs(&mut self) -> bool {
        if has_drawer(self.view.tool) {
            self.view.toggle(self.view.tool);
        }
        let room = self
            .context
            .data(|d| d.get_temp::<bool>(room_id()).unwrap_or(true));
        let shown = self.panel_shown(WorkspacePanel::Presets)
            || self.change_panels(PanelChange::Toggle(WorkspacePanel::Presets));
        self.context.data_mut(|d| d.insert_temp(peek_id(), !room));
        shown
    }
    /// Folds the tab panel. One asked for in a narrow window folds back to
    /// fit and stays unfolded for a wider one; false if the layout could not
    /// be saved.
    fn fold_develop_tabs(&mut self) -> bool {
        let (room, peek) = self.context.data(|d| {
            (
                d.get_temp::<bool>(room_id()).unwrap_or(true),
                d.get_temp::<bool>(peek_id()).unwrap_or(false),
            )
        });
        if peek && !room {
            self.context.data_mut(|d| d.insert_temp(peek_id(), false));
            true
        } else {
            self.change_panels(PanelChange::Toggle(WorkspacePanel::Presets))
        }
    }
    /// Folds the tab panel if its tab shows, or opens it (in place of a
    /// tool's drawer too), as its open tab's rail button and the others do:
    /// for `panels:presets`. False if the layout could not be saved.
    pub(super) fn toggle_develop_tabs(&mut self) -> bool {
        if self.develop_tabs_open() && !has_drawer(self.view.tool) {
            self.fold_develop_tabs()
        } else {
            self.open_develop_tabs()
        }
    }
    /// The tab panel between the adjustments and the photo, if `open`.
    pub(super) fn develop_tab_panel(&mut self, ui: &mut egui::Ui, open: bool) {
        // A tab not drawn this frame (folded, too narrow, or a tool's drawer
        // in its place) lets go of what it was doing: Presets ends a preview
        // it was showing, History saves a snapshot name being typed.
        let drawn = (open && !has_drawer(self.view.tool)).then_some(self.panels.develop_tab);
        if drawn != Some(DevelopTab::Presets) && self.presets.hover.is_some() {
            self.end_preset_hover();
        }
        if drawn != Some(DevelopTab::History) {
            self.commit_snapshot_rename();
        }
        if open {
            self.tab_panel(ui);
        }
    }
    fn tab_panel(&mut self, ui: &mut egui::Ui) {
        let fill = theme::palette(ui.ctx()).gray(22);
        egui::Panel::right("presets")
            .default_size(PANEL_WIDTH)
            .min_size(super::workspace::PRESETS_MIN)
            .max_size(400.)
            .frame(egui::Frame::side_top_panel(ui.style()).fill(fill))
            .show(ui, |ui| {
                let _side = SectionSide::enter(ui, SectionGroup::DevelopLeft);
                if has_drawer(self.view.tool) {
                    // The drawers' labels are short: their controls take the room.
                    let _labels = super::widgets::LabelColumn::enter(ui, DRAWER_LABEL_COLUMN);
                    let enabled =
                        self.document.full().is_some() && !self.view.compare.before_only();
                    egui::ScrollArea::vertical()
                        .id_salt("tool-drawer")
                        .auto_shrink([false; 2])
                        .show(ui, |ui| {
                            ui.add_enabled_ui(enabled, |ui| self.tool_drawer(ui));
                        });
                    return;
                }
                match self.panels.develop_tab {
                    DevelopTab::Presets => self.presets_ui(ui),
                    DevelopTab::History => self.history_tab(ui),
                }
            });
    }
    fn develop_rail(&mut self, ui: &mut egui::Ui, open: bool) -> Option<TabClick> {
        let palette = theme::palette(ui.ctx());
        let mut click = None;
        let tool = self.view.tool;
        let photo = self.document.full().is_some() && !self.view.compare.before_only();
        egui::Panel::right("develop-rail")
            .resizable(false)
            .exact_size(RAIL_WIDTH)
            .frame(egui::Frame::new().fill(palette.gray(22)))
            .show(ui, |ui| {
                let rail = ui.max_rect();
                ui.allocate_rect(rail, Sense::hover());
                // A rail button: its fill, the open one's marker on the
                // window's edge, and whether it was clicked.
                let button = |ui: &mut egui::Ui,
                              y: f32,
                              id: egui::Id,
                              active: bool,
                              enabled: bool,
                              tip: String,
                              paint: &dyn Fn(&egui::Painter, Pos2, egui::Color32, bool)| {
                    let center = Pos2::new(rail.center().x, rail.top() + y);
                    let rect = Rect::from_center_size(center, Vec2::splat(28.));
                    let sense = if enabled {
                        Sense::click()
                    } else {
                        Sense::hover()
                    };
                    let response = ui
                        .interact(rect, id, sense)
                        .on_hover_text(tip)
                        .on_hover_cursor(egui::CursorIcon::PointingHand);
                    let hovered = enabled && response.hovered();
                    if active || hovered {
                        let fill = palette.gray(if active { 54 } else { 44 });
                        ui.painter().rect_filled(rect, 6., fill);
                    }
                    if active {
                        ui.painter().rect_filled(
                            Rect::from_min_size(
                                Pos2::new(rail.right() - 2., rect.top() + 7.),
                                Vec2::new(2., rect.height() - 14.),
                            ),
                            1.,
                            palette.accent(),
                        );
                    }
                    let tint = palette.gray(match (enabled, active, hovered) {
                        (false, ..) => 80,
                        (_, true, _) => 245,
                        (_, _, true) => 230,
                        _ => 145,
                    });
                    paint(ui.painter(), center, tint, active || hovered);
                    response.clicked()
                };
                let mut y = 24.;
                for (tab, icon, name) in TABS {
                    let active = open && !has_drawer(tool) && self.panels.develop_tab == tab;
                    let tip = if active {
                        format!("Hide {name}")
                    } else {
                        format!("Show {name}")
                    };
                    let paint = |p: &egui::Painter, c: Pos2, tint: egui::Color32, _: bool| {
                        icons::paint_at(p, icon, c, 16., tint)
                    };
                    if button(ui, y, rail_id(tab), active, true, tip, &paint) {
                        click = Some(if active {
                            TabClick::Fold
                        } else {
                            TabClick::Open(tab)
                        });
                    }
                    y += 36.;
                }
                // The tools after a gap, their drawers in the panel.
                y += 14.;
                for (t, tip, _) in DRAWER_TOOLS {
                    let paint = |p: &egui::Painter, c: Pos2, _: egui::Color32, strong: bool| {
                        paint_tool_icon(p, t, c, strong)
                    };
                    let id = egui::Id::new(("develop-rail-tool", tip));
                    if button(ui, y, id, tool == t, photo, tip.to_string(), &paint) {
                        click = Some(TabClick::Tool(t));
                    }
                    y += 36.;
                }
            });
        click
    }
    /// The History tab: the open photo's Snapshots over its History.
    fn history_tab(&mut self, ui: &mut egui::Ui) {
        if self.document.metadata.is_none() {
            ui.add_space(8.);
            ui.label(
                egui::RichText::new("Open a photo to see its snapshots and history.")
                    .size(11.)
                    .color(theme::palette(ui.ctx()).gray(125)),
            );
            return;
        }
        ui.spacing_mut().item_spacing.y = 0.;
        egui::ScrollArea::vertical()
            .id_salt("history-list")
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                self.snapshots_section(ui);
                self.history_section(ui);
            });
    }
}
