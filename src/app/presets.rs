use super::Editor;
use super::dialogs::FileDialog;
use super::widgets::{section, segmented};
use super::worker::Event;
use crate::develop::Recipe;
use eframe::egui::{self, Color32, Sense, Stroke, Vec2};
use std::sync::Arc;
use std::time::{Duration, Instant};

impl Editor {
    pub(super) fn reload_presets(&self, ctx: &egui::Context) {
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let library = crate::presets::load_library();
            let _ = tx.send(Event::XmpLibrary(Arc::new(library)));
            ctx.request_repaint();
        });
    }
    pub(super) fn refresh_preset_support(&mut self) {
        let had_preview = self.presets.preview.take().is_some();
        self.presets.hover = None;
        if had_preview {
            self.schedule();
        }
        let Some(m) = &self.document.metadata else {
            self.presets.issues.clear();
            return;
        };
        let base = Recipe::with_profiles(m, &self.document.profiles);
        self.presets.issues = self
            .presets
            .library
            .presets
            .iter()
            .map(|p| {
                p.apply(&base, m, &self.document.profiles, None)
                    .err()
                    .map(|e| format!("{e:#}"))
            })
            .collect();
    }
    pub(super) fn presets_ui(&mut self, ui: &mut egui::Ui) {
        let library = self.presets.library.clone();
        let mut clicked = None;
        let mut hovered = None;
        let mut import = false;
        ui.spacing_mut().item_spacing.y = 0.;
        egui::ScrollArea::vertical()
            .id_salt("preset-list")
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                section(ui, "Presets", false, |ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(4., 4.);
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.presets.filter)
                                .hint_text("Search presets")
                                .font(egui::FontId::proportional(12.))
                                .desired_width(ui.available_width() - 26.),
                        );
                        let (rect, response) =
                            ui.allocate_exact_size(Vec2::splat(22.), Sense::click());
                        let color = Color32::from_gray(if response.hovered() { 235 } else { 160 });
                        if response.hovered() {
                            ui.painter().rect_filled(rect, 3., Color32::from_gray(50));
                        }
                        let c = rect.center();
                        ui.painter().line_segment(
                            [c - Vec2::new(5., 0.), c + Vec2::new(5., 0.)],
                            Stroke::new(1.5, color),
                        );
                        ui.painter().line_segment(
                            [c - Vec2::new(0., 5.), c + Vec2::new(0., 5.)],
                            Stroke::new(1.5, color),
                        );
                        if response
                            .on_hover_text("Import Lightroom presets (.xmp)")
                            .clicked()
                        {
                            import = true;
                        }
                    });
                    let mut show = match (self.presets.favorites_only, self.presets.compatible_only)
                    {
                        (true, _) => 1,
                        (false, true) => 2,
                        _ => 0,
                    };
                    let w = ui.available_width();
                    if segmented(
                        ui,
                        &mut show,
                        &[(0, "All"), (1, "Favorites"), (2, "Compatible")],
                        w,
                    ) {
                        self.presets.favorites_only = show == 1;
                        self.presets.compatible_only = show == 2;
                    }
                    let available = self.presets.issues.iter().filter(|e| e.is_none()).count();
                    ui.label(
                        egui::RichText::new(if self.document.metadata.is_some() {
                            format!(
                                "{} presets · {available} compatible with this camera",
                                library.presets.len()
                            )
                        } else {
                            format!("{} presets · open a RAW to apply", library.presets.len())
                        })
                        .size(10.)
                        .color(Color32::from_gray(125)),
                    );
                    if !library.errors.is_empty() {
                        ui.label(
                            egui::RichText::new(format!(
                                "{} unreadable files",
                                library.errors.len()
                            ))
                            .size(10.)
                            .color(ui.visuals().warn_fg_color),
                        )
                        .on_hover_text(library.errors.join("\n"));
                    }
                    ui.add_space(2.);
                    let query = self.presets.filter.to_lowercase();
                    let mut groups: std::collections::BTreeMap<String, Vec<usize>> =
                        Default::default();
                    for (i, p) in library.presets.iter().enumerate() {
                        let issue = self.presets.issues.get(i).and_then(Option::as_ref);
                        if self.presets.compatible_only && issue.is_some() {
                            continue;
                        }
                        if self.presets.favorites_only && !self.presets.favorites.contains(&p.id) {
                            continue;
                        }
                        if !crate::presets::display_name(&format!("{} {}", p.group, p.name))
                            .to_lowercase()
                            .contains(&query)
                        {
                            continue;
                        }
                        groups.entry(p.group.clone()).or_default().push(i);
                    }
                    if groups.is_empty() && !library.presets.is_empty() {
                        ui.weak("No presets match these filters.");
                        if ui.small_button("Clear filters").clicked() {
                            self.presets.filter.clear();
                            self.presets.favorites_only = false;
                            self.presets.compatible_only = false;
                        }
                    }
                    ui.spacing_mut().item_spacing.y = 0.;
                    let force_open = !query.is_empty() || self.presets.favorites_only;
                    for (group, indices) in groups {
                        let id = ui.make_persistent_id(("preset-group", &group));
                        let open = force_open
                            || ui.ctx().data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
                        if list_row(ui, &group, Some(indices.len()), 0, Some(open), false, true)
                            .clicked()
                            && !force_open
                        {
                            ui.ctx().data_mut(|d| d.insert_temp(id, !open));
                        }
                        if !open {
                            continue;
                        }
                        for i in indices {
                            let p = &library.presets[i];
                            let issue = self.presets.issues.get(i).and_then(Option::as_ref);
                            // Incompatible presets are grayed out but still apply
                            // everything they can, as in Lightroom.
                            let enabled = self.document.full().is_some();
                            let favorite = self.presets.favorites.contains(&p.id);
                            let response = list_row(
                                ui,
                                &crate::presets::display_name(&p.name),
                                None,
                                1,
                                None,
                                self.presets.selected == p.id,
                                enabled && issue.is_none(),
                            );
                            // Favorite star on the right, shown on hover or when set.
                            let star = egui::Rect::from_center_size(
                                egui::pos2(response.rect.right() - 12., response.rect.center().y),
                                Vec2::splat(16.),
                            );
                            let star_response =
                                ui.interact(star, ui.id().with(("favorite", i)), Sense::click());
                            if favorite || response.hovered() || star_response.hovered() {
                                ui.painter().text(
                                    star.center(),
                                    egui::Align2::CENTER_CENTER,
                                    "★",
                                    egui::FontId::proportional(12.),
                                    Color32::from_gray(if star_response.hovered() {
                                        240
                                    } else if favorite {
                                        200
                                    } else {
                                        110
                                    }),
                                );
                            }
                            if star_response
                                .on_hover_text(if favorite {
                                    "Remove from favorites"
                                } else {
                                    "Add to favorites"
                                })
                                .clicked()
                            {
                                if favorite {
                                    self.presets.favorites.remove(&p.id);
                                } else {
                                    self.presets.favorites.insert(p.id.clone());
                                }
                                if let Err(e) =
                                    crate::presets::save_favorites(&self.presets.favorites)
                                {
                                    self.status = e.to_string();
                                }
                                continue;
                            }
                            if enabled && response.clicked() {
                                clicked = Some(i);
                            }
                            if response.hovered() && enabled {
                                hovered = Some(i);
                            }
                            let detail = match issue {
                                Some(issue) => format!(
                                    "{}\nNot fully compatible: {issue}\nClick to apply the supported settings",
                                    p.name
                                ),
                                None => format!(
                                    "{}\nClick to apply · hover to preview\n{}",
                                    p.name,
                                    p.notes.join("\n")
                                ),
                            };
                            response.on_hover_text(detail);
                        }
                    }
                });
                self.history_section(ui);
            });
        if import {
            self.dialog(FileDialog::ImportXmp, &ui.ctx().clone());
        }
        if let Some(i) = clicked {
            self.presets.preview = None;
            self.presets.hover = None;
            if let Some(m) = &self.document.metadata {
                match library.presets[i].apply_lenient(
                    &self.document.recipe,
                    m,
                    &self.document.profiles,
                    self.document.full().map(|image| image.as_ref()),
                ) {
                    Ok((r, skipped)) => {
                        self.document.recipe = r;
                        self.presets.selected = library.presets[i].id.clone();
                        self.status = if skipped.is_empty() {
                            format!("Applied {}", library.presets[i].name)
                        } else {
                            format!(
                                "Applied {} · skipped: {}",
                                library.presets[i].name,
                                skipped.join("; ")
                            )
                        };
                    }
                    Err(e) => self.status = format!("Preset not applied: {e:#}"),
                }
            }
        } else if let Some(i) = hovered {
            if self.presets.hover.as_ref().is_none_or(|(old, _)| *old != i) {
                if self.presets.preview.take().is_some() {
                    self.schedule();
                }
                self.presets.hover = Some((i, Instant::now()));
            }
            if self.presets.preview.is_none()
                && self
                    .presets
                    .hover
                    .as_ref()
                    .is_some_and(|(_, t)| t.elapsed() > Duration::from_millis(300))
                && let Some(m) = &self.document.metadata
                && let Ok((r, _)) = library.presets[i].apply_lenient(
                    &self.document.recipe,
                    m,
                    &self.document.profiles,
                    self.document.full().map(|image| image.as_ref()),
                )
            {
                self.presets.preview = Some(r);
                self.schedule();
            }
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        } else {
            self.presets.hover = None;
            if self.presets.preview.take().is_some() {
                self.schedule();
            }
        }
    }
}

/// A Lightroom-style list row for preset folders and presets: fixed height,
/// indent, optional disclosure triangle and right-aligned count.
fn list_row(
    ui: &mut egui::Ui,
    text: &str,
    count: Option<usize>,
    depth: usize,
    open: Option<bool>,
    selected: bool,
    enabled: bool,
) -> egui::Response {
    use egui::{Align2, FontId, Pos2};
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.), Sense::click());
    if selected || (enabled && response.hovered()) {
        ui.painter().rect_filled(
            rect,
            2.,
            if selected {
                Color32::from_rgb(47, 58, 66)
            } else {
                Color32::from_gray(43)
            },
        );
    }
    let x = rect.left() + 8. + depth as f32 * 16.;
    let y = rect.center().y;
    if let Some(open) = open {
        let c = Pos2::new(x + 3., y);
        let triangle = if open {
            vec![
                c + Vec2::new(-4., -2.),
                c + Vec2::new(4., -2.),
                c + Vec2::new(0., 3.),
            ]
        } else {
            vec![
                c + Vec2::new(-2., -4.),
                c + Vec2::new(3., 0.),
                c + Vec2::new(-2., 4.),
            ]
        };
        ui.painter().add(egui::Shape::convex_polygon(
            triangle,
            Color32::from_gray(140),
            Stroke::NONE,
        ));
    }
    let text_left = x + if open.is_some() { 14. } else { 6. };
    let right = rect.right() - if count.is_some() { 36. } else { 26. };
    let galley = egui::WidgetText::from(text).into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        (right - text_left).max(1.),
        FontId::proportional(12.),
    );
    ui.painter().galley(
        Pos2::new(text_left, y - galley.size().y / 2.),
        galley,
        Color32::from_gray(if !enabled {
            95
        } else if selected {
            240
        } else {
            195
        }),
    );
    if let Some(count) = count {
        ui.painter().text(
            Pos2::new(rect.right() - 10., y),
            Align2::RIGHT_CENTER,
            count.to_string(),
            FontId::proportional(10.),
            Color32::from_gray(120),
        );
    }
    response
}

impl Editor {
    /// Lightroom's History panel: this session's steps, newest first, over
    /// the photo's imported Lightroom steps. Clicking a step of this session
    /// goes back (or forward) to it; later steps stay until the next edit.
    fn history_section(&mut self, ui: &mut egui::Ui) {
        if self.document.metadata.is_none() {
            return;
        }
        let mut go_to = None;
        let mut lightroom = None;
        let (steps, applied) = self.document.history.steps();
        section(ui, "History", false, |ui| {
            ui.spacing_mut().item_spacing.y = 0.;
            for (i, step) in steps.iter().enumerate().rev() {
                let n = i + 1;
                if history_row(ui, &step.name, &step.value, n == applied, n > applied).clicked() {
                    go_to = Some(n);
                }
            }
            let opened = if self.document.lightroom_history.is_empty() {
                "Opened"
            } else {
                "Opened with Lightroom edit"
            };
            if history_row(ui, opened, "", applied == 0, false).clicked() {
                go_to = Some(0);
            }
            if self.document.lightroom_history.is_empty() {
                return;
            }
            ui.add_space(10.);
            ui.label(
                egui::RichText::new("From Lightroom")
                    .size(11.)
                    .color(Color32::from_gray(125)),
            );
            ui.add_space(4.);
            for (i, step) in self.document.lightroom_history.iter().enumerate().rev() {
                let name = if step.name.is_empty() {
                    "Edit"
                } else {
                    step.name.as_str()
                };
                let response = history_row(ui, name, "", false, false);
                let response = match step.created {
                    // Lightroom counts seconds from 2001-01-01 UTC.
                    Some(s) => response
                        .on_hover_text(format!("{} UTC", format_unix(s as i64 + 978_307_200))),
                    None => response,
                };
                if response.clicked() {
                    lightroom = Some(i);
                }
            }
        });
        if let Some(n) = go_to {
            let current = &mut self.document.recipe;
            self.document.history.go_to(n, current);
        }
        if let Some(i) = lightroom
            && let Some(m) = &self.document.metadata
        {
            let step = &self.document.lightroom_history[i];
            match crate::catalog::convert_develop(
                &step.text,
                m,
                &self.document.profiles,
                self.document.full().map(|image| image.as_ref()),
            ) {
                Ok((recipe, skipped)) => {
                    let name = format!("Lightroom: {}", step.name);
                    self.status = if skipped.is_empty() {
                        name.clone()
                    } else {
                        format!("{name} · not rendered: {}", skipped.join(", "))
                    };
                    self.document
                        .history
                        .label(super::history::Step::new(name, ""));
                    self.document.recipe = recipe;
                }
                Err(e) => self.status = format!("History step not applied: {e:#}"),
            }
        }
    }
}
/// A History row: the step on the left, its value on the right. The current
/// step is highlighted; steps after it (undone) are dimmed, as in Lightroom.
fn history_row(
    ui: &mut egui::Ui,
    name: &str,
    value: &str,
    current: bool,
    undone: bool,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 24.), Sense::click());
    if current {
        ui.painter()
            .rect_filled(rect, 3., Color32::from_rgb(62, 88, 115));
    } else if response.hovered() {
        ui.painter().rect_filled(rect, 3., Color32::from_gray(43));
    }
    let color = Color32::from_gray(if current {
        250
    } else if undone {
        120
    } else {
        205
    });
    let value_width = if value.is_empty() {
        0.
    } else {
        let galley =
            ui.painter()
                .layout_no_wrap(value.to_string(), egui::FontId::proportional(12.), color);
        let width = galley.size().x;
        ui.painter().galley(
            egui::pos2(
                rect.right() - 8. - width,
                rect.center().y - galley.size().y / 2.,
            ),
            galley,
            color,
        );
        width + 16.
    };
    let clip = egui::Rect::from_min_max(
        rect.min,
        egui::pos2(rect.right() - 8. - value_width, rect.bottom()),
    );
    ui.painter().with_clip_rect(clip).text(
        rect.left_center() + Vec2::new(8., 0.),
        egui::Align2::LEFT_CENTER,
        name,
        egui::FontId::proportional(12.),
        color,
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}
/// "2016-07-28 06:14" from Unix seconds.
fn format_unix(seconds: i64) -> String {
    let [year, month, day, hour, minute, _] = crate::time::utc(seconds);
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}")
}
#[cfg(test)]
#[test]
fn formats_lightroom_history_dates() {
    assert_eq!(format_unix(1_469_686_444), "2016-07-28 06:14");
}
