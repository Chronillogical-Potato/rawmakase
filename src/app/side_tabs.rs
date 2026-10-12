//! Develop's tab panel beside the adjustments: Presets, and Snapshots over
//! History, one at a time. Open, its header switches the tab and folds it;
//! folded, a slim rail of tab buttons brings it back. A window too narrow for
//! it and the photo shows it folded until a tab is asked for.
use super::Editor;
use super::icons::{self, Icon};
use super::panels::{DevelopTab, PanelChange, WorkspacePanel};
use super::theme;
use super::widgets::{SectionGroup, SectionSide, segmented};
use eframe::egui::{self, Pos2, Rect, Sense, Vec2};

/// The folded panel's rail.
const RAIL_WIDTH: f32 = 36.;
/// The tab panel's width until the user drags it.
const PANEL_WIDTH: f32 = 245.;
/// The photo's narrowest width before the tab panel folds by itself.
const PHOTO_MIN: f32 = 400.;
/// The tabs, top to bottom on the rail, with their icons and names.
const TABS: [(DevelopTab, Icon, &str); 2] = [
    (DevelopTab::Presets, Icon::Presets, "Presets"),
    (DevelopTab::History, Icon::History, "History"),
];

/// What a click on the panel's header or rail asked for.
enum TabClick {
    /// Show this tab, opening the panel.
    Open(DevelopTab),
    Fold,
}

/// A rail button's id, fixed so tests can find it.
pub(super) fn rail_id(tab: DevelopTab) -> egui::Id {
    egui::Id::new(("develop-rail", tab))
}
/// Whether a tab was asked for while the window had no room for the panel.
fn peek_id() -> egui::Id {
    egui::Id::new("develop-tabs-peek")
}

impl Editor {
    /// The tab panel, or its rail while folded. Drawn after the adjustments,
    /// so it sits between them and the photo. Hidden with the adjustments
    /// when it is folded too, as Tab does.
    pub(super) fn develop_side_tabs(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let shown = self.panel_shown(WorkspacePanel::Presets);
        let width = egui::containers::panel::PanelState::load(&ctx, egui::Id::new("presets"))
            .map_or(PANEL_WIDTH, |state| state.outer_rect.width());
        let room = ui.available_width() - width >= PHOTO_MIN;
        let peek = ctx.data(|d| d.get_temp::<bool>(peek_id()).unwrap_or(false));
        let open = shown && (room || peek);
        let click = if open {
            self.tab_panel(ui)
        } else {
            // Not drawn, the Presets tab ends a preview it was showing.
            if self.presets.hover.is_some() {
                self.end_preset_hover();
            }
            if !shown && !self.panel_shown(WorkspacePanel::Right) {
                return;
            }
            self.develop_rail(ui)
        };
        match click {
            Some(TabClick::Open(tab)) => {
                self.set_develop_tab(tab);
                if !shown {
                    self.change_panels(PanelChange::Toggle(WorkspacePanel::Presets));
                }
                ctx.data_mut(|d| d.insert_temp(peek_id(), !room));
            }
            // Asked for in a narrow window, it folds back to fit; otherwise
            // it stays folded.
            Some(TabClick::Fold) if peek && !room => {
                ctx.data_mut(|d| d.insert_temp(peek_id(), false));
            }
            Some(TabClick::Fold) => {
                self.change_panels(PanelChange::Toggle(WorkspacePanel::Presets));
            }
            None => {}
        }
    }
    fn tab_panel(&mut self, ui: &mut egui::Ui) -> Option<TabClick> {
        let mut click = None;
        egui::Panel::right("presets")
            .default_size(PANEL_WIDTH)
            .min_size(super::workspace::PRESETS_MIN)
            .max_size(400.)
            .show(ui, |ui| {
                let _side = SectionSide::enter(ui, SectionGroup::DevelopLeft);
                click = tab_header(ui, self.panels.develop_tab);
                match self.panels.develop_tab {
                    DevelopTab::Presets => self.presets_ui(ui),
                    DevelopTab::History => self.history_tab(ui),
                }
            });
        click
    }
    fn develop_rail(&mut self, ui: &mut egui::Ui) -> Option<TabClick> {
        let palette = theme::palette(ui.ctx());
        let mut click = None;
        egui::Panel::right("develop-rail")
            .resizable(false)
            .exact_size(RAIL_WIDTH)
            .frame(egui::Frame::new().fill(palette.gray(28)))
            .show(ui, |ui| {
                let rail = ui.max_rect();
                ui.allocate_rect(rail, Sense::hover());
                for (k, (tab, icon, name)) in TABS.into_iter().enumerate() {
                    let center = Pos2::new(rail.center().x, rail.top() + 24. + k as f32 * 36.);
                    let rect = Rect::from_center_size(center, Vec2::splat(28.));
                    let response = ui
                        .interact(rect, rail_id(tab), Sense::click())
                        .on_hover_text(format!("Show {name}"))
                        .on_hover_cursor(egui::CursorIcon::PointingHand);
                    if response.hovered() {
                        ui.painter().rect_filled(rect, 6., palette.gray(44));
                    }
                    let tint = palette.gray(if response.hovered() { 230 } else { 145 });
                    icons::paint_at(ui.painter(), icon, center, 16., tint);
                    if response.clicked() {
                        click = Some(TabClick::Open(tab));
                    }
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

/// The panel's header: the tabs, and the button that folds it.
fn tab_header(ui: &mut egui::Ui, open: DevelopTab) -> Option<TabClick> {
    let palette = theme::palette(ui.ctx());
    let mut click = None;
    ui.add_space(6.);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.;
        let mut tab = open;
        let w = ui.available_width() - 30.;
        let labels = TABS.map(|(tab, _, name)| (tab, name));
        if segmented(ui, &mut tab, &labels, w) {
            click = Some(TabClick::Open(tab));
        }
        let (rect, response) = ui.allocate_exact_size(Vec2::splat(24.), Sense::click());
        if response.hovered() {
            ui.painter().rect_filled(rect, 4., palette.gray(50));
        }
        let tint = palette.gray(if response.hovered() { 230 } else { 150 });
        icons::paint_at(ui.painter(), Icon::ChevronRight, rect.center(), 14., tint);
        if response
            .on_hover_text("Hide Presets and History")
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
        {
            click = Some(TabClick::Fold);
        }
    });
    ui.add_space(2.);
    click
}
