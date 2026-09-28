use super::Editor;
use super::bulk_import::ImportKind;
use super::dialogs::FileDialog;
use super::state::Tool;
use super::widgets::{
    adjustment_section, parametric_curve_ui, segmented, slider, slider_with, tone_curve_ui,
    toolbar_action,
};
use super::worker::AutoKind;
use crate::app::icons::{self, Icon};
use crate::app::theme;
use crate::develop::{Recipe, TEMPERATURE_MAX, TEMPERATURE_MIN, TINT_LIMIT};
use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};

impl Editor {
    /// Lightroom-style histogram: filled channels whose overlaps mix to
    /// cyan, magenta, yellow and gray, with clipping indicators in the corners.
    pub(super) fn histogram_ui(&mut self, ui: &mut egui::Ui) {
        let (rect, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 96.), Sense::hover());
        let painter = ui.painter().clone();
        painter.rect_filled(rect, 2., theme::gray(20));
        for i in 1..5 {
            let x = rect.left() + i as f32 / 5. * rect.width();
            painter.line_segment(
                [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
                Stroke::new(1., theme::gray(32)),
            );
        }
        let h = self.preview.histogram;
        // Light smoothing, and scaling that ignores the clipped end bins.
        let smooth = |c: usize, i: usize| {
            let at = |j: isize| h[c][j.clamp(0, 255) as usize] as f32;
            let i = i as isize;
            (at(i - 1) + 2. * at(i) + at(i + 1)) / 4.
        };
        let max = (2..254)
            .flat_map(|i| (0..3).map(move |c| (c, i)))
            .map(|(c, i)| smooth(c, i))
            .fold(1., f32::max);
        let plot = rect.shrink2(Vec2::new(0., 4.));
        let bar = plot.width() / 256.;
        let colors = [
            Color32::from_rgb(196, 58, 52),
            Color32::from_rgb(62, 170, 70),
            Color32::from_rgb(56, 104, 220),
        ];
        let pair = |a: usize, b: usize| match (a.min(b), a.max(b)) {
            (0, 1) => Color32::from_rgb(190, 176, 60),
            (1, 2) => Color32::from_rgb(58, 168, 186),
            _ => Color32::from_rgb(170, 70, 170),
        };
        for i in 0..256 {
            let mut v: Vec<(f32, usize)> = (0..3)
                .map(|c| ((smooth(c, i) / max).sqrt().min(1.) * plot.height(), c))
                .collect();
            v.sort_by(|a, b| a.0.total_cmp(&b.0));
            let x = plot.left() + i as f32 * bar;
            let segment = |from: f32, to: f32, color: Color32| {
                if to > from {
                    painter.rect_filled(
                        Rect::from_min_max(
                            Pos2::new(x, plot.bottom() - to),
                            Pos2::new(x + bar + 0.5, plot.bottom() - from),
                        ),
                        0.,
                        color,
                    );
                }
            };
            segment(0., v[0].0, theme::gray(150));
            segment(v[0].0, v[1].0, pair(v[1].1, v[2].1));
            segment(v[1].0, v[2].0, colors[v[2].1]);
        }
        // Clipping triangles: lit when pixels sit in the end bins; click toggles J.
        let total: u32 = h[1].iter().sum::<u32>().max(1);
        for (left, bin) in [(true, 0usize), (false, 255usize)] {
            let clipped = (0..3).any(|c| h[c][bin] as f32 / total as f32 > 0.001);
            let corner = if left {
                rect.left_top() + Vec2::new(6., 6.)
            } else {
                rect.right_top() + Vec2::new(-6., 6.)
            };
            let dir = if left { 1. } else { -1. };
            let hit = Rect::from_center_size(corner + Vec2::new(4. * dir, 3.), Vec2::splat(16.));
            let response = ui
                .interact(hit, ui.id().with(("clip", left)), Sense::click())
                .on_hover_text(if left {
                    "Shadow clipping · J shows clipped areas"
                } else {
                    "Highlight clipping · J shows clipped areas"
                });
            let color = if clipped {
                theme::gray(235)
            } else {
                theme::gray(if self.view.clipping || response.hovered() {
                    150
                } else {
                    80
                })
            };
            painter.add(egui::Shape::convex_polygon(
                vec![
                    corner,
                    corner + Vec2::new(9. * dir, 0.),
                    corner + Vec2::new(0., 7.),
                ],
                color,
                if self.view.clipping {
                    Stroke::new(1., Color32::WHITE)
                } else {
                    Stroke::NONE
                },
            ));
            if response.clicked() {
                self.view.clipping = !self.view.clipping;
            }
        }
        let exif = self
            .document
            .metadata
            .as_ref()
            .map_or_else(String::new, |m| {
                let shutter = if m.shutter > 0. && m.shutter < 1. {
                    format!("1/{:.0} sec", 1. / m.shutter)
                } else {
                    format!("{} sec", m.shutter)
                };
                format!(
                    "ISO {}     {} mm     f/{}     {shutter}",
                    m.iso, m.focal, m.aperture
                )
            });
        ui.vertical_centered(|ui| {
            ui.label(egui::RichText::new(exif).size(11.).color(theme::gray(170)))
                .on_hover_text("Output histogram of the whole photo");
        });
    }
    /// Lightroom's tool strip: Crop, Remove and Masking, with the open tool's drawer
    /// below it.
    fn tool_strip(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.);
        const TOOLS: [(Tool, &str, &str); 3] = [
            (Tool::Crop, "Crop", "Crop & Straighten · R"),
            (Tool::Remove, "Remove", "Spot Removal: Heal and Clone · Q"),
            (Tool::Mask, "Masking", "Masking · Shift+W"),
        ];
        let (strip, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 28.), Sense::hover());
        let width = (strip.width() - 8.) / 3.;
        for (k, (tool, label, tip)) in TOOLS.into_iter().enumerate() {
            let rect = Rect::from_min_size(
                strip.min + Vec2::new(k as f32 * (width + 4.), 0.),
                Vec2::new(width, strip.height()),
            );
            let response = ui
                .interact(rect, ui.id().with(("tool", label)), Sense::click())
                .on_hover_text(tip);
            let active = self.view.is(tool);
            ui.painter().rect_filled(
                rect,
                3.,
                theme::gray(if active {
                    72
                } else if response.hovered() {
                    50
                } else {
                    38
                }),
            );
            let icon = rect.left_center() + Vec2::new(16., 0.);
            match tool {
                Tool::Crop => crop_icon(ui.painter(), icon, active),
                Tool::Remove => heal_icon(ui.painter(), icon, active),
                _ => mask_icon(ui.painter(), icon, active),
            }
            ui.painter().text(
                rect.left_center() + Vec2::new(30., 0.),
                egui::Align2::LEFT_CENTER,
                label,
                egui::FontId::proportional(12.),
                theme::gray(if active { 245 } else { 200 }),
            );
            if response
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                self.view.toggle(tool);
            }
        }
        let drawer = |ui: &mut egui::Ui, add: &mut dyn FnMut(&mut egui::Ui)| {
            ui.add_space(4.);
            egui::Frame::new()
                .fill(theme::gray(40))
                .corner_radius(3.)
                .inner_margin(egui::Margin {
                    left: 0,
                    right: 8,
                    top: 8,
                    bottom: 8,
                })
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(4., 6.);
                    add(ui)
                });
        };
        match self.view.tool {
            Tool::Remove => {
                return drawer(ui, &mut |ui| {
                    experimental(ui);
                    self.retouch_panel(ui)
                });
            }
            Tool::Mask => {
                return drawer(ui, &mut |ui| {
                    experimental(ui);
                    self.mask_panel(ui)
                });
            }
            Tool::Crop => {}
            _ => return,
        }
        ui.add_space(4.);
        let r = &mut self.document.recipe;
        egui::Frame::new()
            .fill(theme::gray(40))
            .corner_radius(3.)
            .inner_margin(egui::Margin {
                left: 0,
                right: 8,
                top: 8,
                bottom: 8,
            })
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = Vec2::new(4., 6.);
                control_row(ui, "Aspect", |ui| {
                    const ASPECTS: [(f32, &str); 8] = [
                        (-1., "Original"),
                        (0., "Free"),
                        (1., "1 x 1"),
                        (1.25, "4 x 5"),
                        (1.4, "5 x 7"),
                        (1.5, "2 x 3"),
                        (4. / 3., "3 x 4"),
                        (16. / 9., "16 x 9"),
                    ];
                    egui::ComboBox::from_id_salt("crop-aspect")
                        .width(ui.available_width())
                        .selected_text(
                            ASPECTS
                                .iter()
                                .find(|(a, _)| *a == self.view.aspect)
                                .map_or("Custom", |(_, name)| *name),
                        )
                        .show_ui(ui, |ui| {
                            for (aspect, name) in ASPECTS {
                                ui.selectable_value(&mut self.view.aspect, aspect, name);
                            }
                        });
                });
                slider(ui, "Angle", &mut r.straighten, -45. ..=45., 0.);
                control_row(ui, "Orientation", |ui| {
                    let w = ui.available_width();
                    let mut none = usize::MAX;
                    segmented(
                        ui,
                        &mut none,
                        &[
                            (0, "Rotate L"),
                            (1, "Rotate R"),
                            (2, "Flip H"),
                            (3, "Flip V"),
                        ],
                        w,
                    );
                    match none {
                        0 => {
                            r.rotation = (r.rotation + 3) % 4;
                            r.crop = [0., 0., 1., 1.];
                        }
                        1 => {
                            r.rotation = (r.rotation + 1) % 4;
                            r.crop = [0., 0., 1., 1.];
                        }
                        2 => r.flip_x = !r.flip_x,
                        3 => r.flip_y = !r.flip_y,
                        _ => {}
                    }
                });
                ui.horizontal(|ui| {
                    ui.add_space(83.);
                    ui.label(
                        egui::RichText::new("Drag the frame on the photo; changes apply live.")
                            .size(10.)
                            .color(theme::gray(125)),
                    );
                });
                ui.horizontal(|ui| {
                    ui.add_space(83.);
                    let w = (ui.available_width() - 4.) / 2.;
                    if ui
                        .add_sized([w, 22.], egui::Button::new("Reset"))
                        .on_hover_text("Remove the crop, straightening, rotation and flips")
                        .clicked()
                    {
                        r.crop = [0., 0., 1., 1.];
                        r.straighten = 0.;
                        r.rotation = 0;
                        r.flip_x = false;
                        r.flip_y = false;
                    }
                    if ui
                        .add_sized(
                            [w, 22.],
                            egui::Button::new(
                                egui::RichText::new("Done").color(theme::on_accent_text(245)),
                            )
                            .fill(theme::accent()),
                        )
                        .on_hover_text("Finish cropping · Enter or R")
                        .clicked()
                    {
                        self.view.tool = Tool::None;
                    }
                });
            });
    }
    pub(super) fn controls(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().item_spacing = Vec2::new(4., 3.);
        ui.spacing_mut().button_padding = Vec2::new(6., 2.);
        ui.spacing_mut().interact_size.y = 20.;
        self.histogram_ui(ui);
        self.tool_strip(ui);
        ui.add_space(6.);
        let mut import_profiles = false;
        let mut import_lens = false;
        let mut import_folder = None;
        let mut import_adobe = false;
        // Cached per camera: this scans Adobe's profile folders.
        let adobe_key = self
            .document
            .metadata
            .as_ref()
            .map(|m| egui::Id::new(("adobe-profiles", &m.make, &m.model)));
        let adobe: Vec<std::path::PathBuf> = match (adobe_key, &self.document.metadata) {
            (Some(key), Some(m)) => ui.ctx().data_mut(|d| {
                d.get_temp_mut_or_insert_with(key, || adobe_camera_profiles(m))
                    .clone()
            }),
            _ => Vec::new(),
        };
        let metadata = self.document.metadata.clone();
        let profiles = self.document.profiles.clone();
        let profile_errors = self.document.profile_errors.clone();
        let histogram = self.preview.histogram;
        // Auto needs the decoded photo, and runs one estimate at a time.
        let auto_ready = self.document.full().is_some() && !self.document.auto.is_running();
        let auto_in_effect = self.auto_in_effect();
        let mut auto_request = None;
        let view = &mut self.view;
        let r = &mut self.document.recipe;

        if adjustment_section(ui, "Basic", |ui| {
            // Auto, and Black & White as an on/off toggle, in place of Lightroom's
            // Treatment switcher.
            let shortcut = if cfg!(target_os = "macos") {
                "⌘⇧U"
            } else {
                "Ctrl+Shift+U"
            };
            // Right-aligned, B&W at the panel's edge and Auto to its left; styled as the
            // toolbar's Before and Clipping.
            let (row, _) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), 32.), Sense::hover());
            ui.scope_builder(
                egui::UiBuilder::new()
                    .max_rect(row)
                    .layout(egui::Layout::right_to_left(egui::Align::Center)),
                |ui| {
                    ui.spacing_mut().item_spacing.x = 4.;
                    let mono = r.effects.monochrome;
                    if toolbar_action(ui, "B&W", 52., mono, true, 0)
                        .on_hover_text(if mono {
                            "Black & White is on"
                        } else {
                            "Convert to Black & White"
                        })
                        .clicked()
                    {
                        r.effects.monochrome = !mono;
                    }
                    let tip = if auto_in_effect {
                        "Auto settings are applied".to_owned()
                    } else {
                        format!("Set white balance and tone automatically · {shortcut}")
                    };
                    let auto =
                        toolbar_action(ui, "Auto", 52., false, auto_ready && !auto_in_effect, 0);
                    if auto.on_hover_text(tip).clicked() {
                        auto_request = Some(AutoKind::Settings);
                    }
                },
            );
            let old_profile = r.profile.clone();
            // From engine 4 the matrix path renders through the DNG default look.
            let matrix = if r.engine >= 4 {
                "Default (camera matrix)"
            } else {
                "Camera matrix"
            };
            control_row(ui, "Profile", |ui| {
                egui::ComboBox::from_id_salt("camera-profile")
                    .width(ui.available_width())
                    .selected_text(r.profile.as_ref().map_or(matrix, |p| p.name.as_str()))
                    .show_ui(ui, |ui| {
                        if ui.selectable_value(&mut r.profile, None, matrix).changed() {
                            r.engine = r.engine.max(3);
                        }
                        for profile in &profiles {
                            if ui
                                .selectable_value(&mut r.profile, Some(profile.clone()), &profile.name)
                                .on_hover_text(&profile.copyright)
                                .changed()
                            {
                                r.engine = r.engine.max(3);
                            }
                        }
                        ui.separator();
                        if !adobe.is_empty()
                            && ui
                                .button("Import Adobe profiles for this camera")
                                .on_hover_text(format!(
                                    "Adds Adobe Standard and the Camera Matching profiles from Lightroom's installation ({} files), which Adobe Color and the other Adobe looks build on.",
                                    adobe.len()
                                ))
                                .clicked()
                        {
                            import_adobe = true;
                            ui.close();
                        }
                        if ui
                            .button("Import profiles…")
                            .on_hover_text("Import DCP camera profiles and XMP look profiles. Select the matching base DCP and XMP together.")
                            .clicked()
                        {
                            import_profiles = true;
                            ui.close();
                        }
                        if ui
                            .button("Import profiles from folder…")
                            .on_hover_text("Import every DCP profile and XMP look in a folder and its subfolders")
                            .clicked()
                        {
                            import_folder = Some(ImportKind::CameraProfiles);
                            ui.close();
                        }
                    });
            });
            if !profile_errors.is_empty() {
                ui.horizontal(|ui| {
                    ui.add_space(88.);
                    ui.small(format!("{} profiles unavailable", profile_errors.len()))
                        .on_hover_text(profile_errors.join("\n"));
                    if !adobe.is_empty() && ui.small_button("Import Adobe base profile").clicked() {
                        import_adobe = true;
                    }
                });
            }
            if old_profile != r.profile
                && let Some(m) = &metadata
            {
                if r.profile.as_ref().is_some_and(|p| p.enhanced.is_some()) {
                    r.profile_tone = true;
                    r.reference_curves = true;
                    r.wide_gamut_curves = true;
                }
                r.use_camera_baseline(m);
                r.sync_white_balance_controls(m);
            }
            ui.add_space(4.);
            let as_shot = metadata.as_ref().map(|m| {
                let mut shot = r.clone();
                shot.reset_white_balance(m);
                (shot.temperature, shot.tint)
            });
            control_row(ui, "WB", |ui| {
                let current = (r.temperature, r.tint);
                let near =
                    |(t, n): (f32, f32)| (t - current.0).abs() < 1. && (n - current.1).abs() < 0.5;
                let selected = if as_shot.is_some_and(near) {
                    "As Shot"
                } else {
                    WB_PRESETS
                        .iter()
                        .find(|(_, t, n)| near((*t, *n)))
                        .map_or("Custom", |(name, _, _)| *name)
                };
                egui::ComboBox::from_id_salt("white-balance")
                    .width((ui.available_width() - 30.).max(80.))
                    .selected_text(selected)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(selected == "As Shot", "As Shot")
                            .clicked()
                            && let Some(m) = &metadata
                        {
                            r.wb = [1.; 3];
                            r.reset_white_balance(m);
                        }
                        if ui
                            .add_enabled(auto_ready, egui::Button::selectable(false, "Auto"))
                            .on_hover_text("Make the photo's near-neutral areas neutral")
                            .clicked()
                        {
                            auto_request = Some(AutoKind::WhiteBalance);
                        }
                        for (name, temperature, tint) in WB_PRESETS {
                            if ui.selectable_label(selected == name, name).clicked()
                                && let Some(m) = &metadata
                            {
                                r.temperature = temperature;
                                r.tint = tint;
                                r.update_wb(m);
                            }
                        }
                        ui.add_enabled(
                            false,
                            egui::Button::selectable(selected == "Custom", "Custom"),
                        );
                    });
                let (rect, response) = ui.allocate_exact_size(Vec2::new(26., 20.), Sense::click());
                let picking = view.is(Tool::WhiteBalance);
                if picking || response.hovered() {
                    ui.painter()
                        .rect_filled(rect, 3., theme::gray(if picking { 72 } else { 50 }));
                }
                eyedropper_icon(ui.painter(), rect.center(), picking || response.hovered());
                if response
                    .on_hover_text("White balance selector (W): click a neutral area of the photo")
                    .clicked()
                {
                    view.toggle(Tool::WhiteBalance);
                }
            });
            let old = (r.temperature, r.tint);
            slider(
                ui,
                "Temp",
                &mut r.temperature,
                TEMPERATURE_MIN..=TEMPERATURE_MAX,
                6500.,
            );
            slider(ui, "Tint", &mut r.tint, -TINT_LIMIT..=TINT_LIMIT, 0.);
            if old != (r.temperature, r.tint)
                && let Some(m) = &metadata
            {
                r.update_wb(m);
            }
            subheading(ui, "Tone");
            slider_with(
                ui,
                "Exposure",
                &mut r.exposure,
                -5. ..=5.,
                0.,
                Some((1., 2)),
                None,
            );
            slider(ui, "Contrast", &mut r.contrast, -1. ..=1., 0.);
            slider(ui, "Highlights", &mut r.highlights, -1. ..=1., 0.);
            slider(ui, "Shadows", &mut r.shadows, -1. ..=1., 0.);
            slider(ui, "Whites", &mut r.whites, -1. ..=1., 0.);
            slider(ui, "Blacks", &mut r.blacks, -1. ..=1., 0.);
            subheading(ui, "Presence");
            slider(ui, "Texture", &mut r.effects.texture, -1. ..=1., 0.);
            slider(ui, "Clarity", &mut r.effects.clarity, -1. ..=1., 0.);
            slider(ui, "Dehaze", &mut r.effects.dehaze, -1. ..=1., 0.);
            slider(ui, "Vibrance", &mut r.vibrance, -1. ..=1., 0.);
            slider(ui, "Saturation", &mut r.saturation, -1. ..=1., 0.);
        }) {
            r.wb = [1.; 3];
            r.tint = 0.;
            if let Some(m) = &metadata {
                r.reset_white_balance(m);
            }
            r.effects.monochrome = false;
            r.effects.texture = 0.;
            r.effects.clarity = 0.;
            r.effects.dehaze = 0.;
            r.vibrance = 0.;
            r.saturation = 0.;
            r.exposure = Recipe::default().exposure;
            r.contrast = 0.;
            r.highlights = 0.;
            r.shadows = 0.;
            r.whites = 0.;
            r.blacks = 0.;
        }

        if adjustment_section(ui, "Tone Curve", |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.;
                segmented(
                    ui,
                    &mut view.parametric_curve,
                    &[(true, "Parametric"), (false, "Point")],
                    128.,
                );
                if !view.parametric_curve {
                    let w = ui.available_width();
                    segmented(
                        ui,
                        &mut view.selected_curve,
                        &[(0, "RGB"), (1, "R"), (2, "G"), (3, "B")],
                        w,
                    );
                }
            });
            ui.add_space(4.);
            if view.parametric_curve {
                parametric_curve_ui(ui, &mut r.effects, &histogram);
                subheading(ui, "Region");
                for (i, name) in [
                    (3, "Highlights"),
                    (2, "Lights"),
                    (1, "Darks"),
                    (0, "Shadows"),
                ] {
                    ui.push_id(("parametric", i), |ui| {
                        slider(ui, name, &mut r.effects.parametric[i], -1. ..=1., 0.)
                    });
                }
            } else {
                ui.push_id(view.selected_curve, |ui| {
                    tone_curve_ui(
                        ui,
                        if view.selected_curve == 0 {
                            &mut r.curve
                        } else {
                            &mut r.effects.channels[view.selected_curve - 1]
                        },
                        &histogram,
                        view.selected_curve,
                    );
                });
            }
            subheading(ui, "Levels");
            slider(
                ui,
                "Black point",
                &mut r.black_point,
                0. ..=r.white_point - 0.01,
                0.,
            );
            slider(
                ui,
                "White point",
                &mut r.white_point,
                r.black_point + 0.01..=1.,
                1.,
            );
            slider(ui, "Midtone", &mut r.midtone, 0.1..=4., 1.);
        }) {
            r.black_point = 0.;
            r.white_point = 1.;
            r.midtone = 1.;
            r.curve = Recipe::default().curve;
            r.effects.channels = std::array::from_fn(|_| Default::default());
            r.effects.parametric = [0.; 4];
        }

        let mixer_title = if r.effects.monochrome {
            "B&W"
        } else {
            "Color Mixer"
        };
        if adjustment_section(ui, mixer_title, |ui| {
            if r.effects.monochrome {
                subheading(ui, "Black & White Mix");
                for (i, name) in BANDS.iter().enumerate() {
                    ui.push_id(("bw", i), |ui| {
                        slider_with(
                            ui,
                            name,
                            &mut r.effects.gray_mix[i],
                            -1. ..=1.,
                            0.,
                            None,
                            Some((theme::gray(40), band_color(i))),
                        )
                    });
                }
                return;
            }
            control_row(ui, "Mixer", |ui| {
                let w = ui.available_width();
                segmented(
                    ui,
                    &mut view.mixer_color,
                    &[(false, "HSL"), (true, "Color")],
                    w,
                );
            });
            if view.mixer_color {
                ui.horizontal(|ui| {
                    for (i, band) in BANDS.iter().enumerate() {
                        let (rect, response) =
                            ui.allocate_exact_size(Vec2::splat(27.), Sense::click());
                        ui.painter().circle_filled(rect.center(), 7., band_color(i));
                        if view.selected_band == i {
                            ui.painter().circle_stroke(
                                rect.center(),
                                10.,
                                Stroke::new(1.5, theme::gray(215)),
                            );
                        }
                        if response.on_hover_text(*band).clicked() {
                            view.selected_band = i;
                        }
                    }
                });
                let i = view.selected_band;
                ui.push_id(("hsl", i), |ui| {
                    for (c, name) in ["Hue", "Saturation", "Luminance"].into_iter().enumerate() {
                        slider_with(
                            ui,
                            name,
                            &mut r.hsl[i][c],
                            -1. ..=1.,
                            0.,
                            None,
                            Some(hsl_gradient(i, c)),
                        );
                    }
                });
            } else {
                control_row(ui, "Adjust", |ui| {
                    let w = ui.available_width();
                    segmented(
                        ui,
                        &mut view.mixer_adjust,
                        &[(0, "Hue"), (1, "Sat"), (2, "Lum"), (3, "All")],
                        w,
                    );
                });
                let channels = if view.mixer_adjust == 3 {
                    0..3
                } else {
                    view.mixer_adjust..view.mixer_adjust + 1
                };
                for c in channels {
                    if view.mixer_adjust == 3 {
                        subheading(ui, ["Hue", "Saturation", "Luminance"][c]);
                    }
                    for (i, name) in BANDS.iter().enumerate() {
                        ui.push_id(("hsl-all", c, i), |ui| {
                            slider_with(
                                ui,
                                name,
                                &mut r.hsl[i][c],
                                -1. ..=1.,
                                0.,
                                None,
                                Some(hsl_gradient(i, c)),
                            );
                        });
                    }
                }
            }
        }) {
            if r.effects.monochrome {
                r.effects.gray_mix = [0.; 8];
            } else {
                r.hsl = [[0.; 3]; 8];
            }
        }

        if adjustment_section(ui, "Color Grading", |ui| {
            ui.horizontal(|ui| {
                let w = ui.available_width();
                segmented(
                    ui,
                    &mut view.selected_grade,
                    &[
                        (0, "Shadows"),
                        (1, "Midtones"),
                        (2, "Highlights"),
                        (3, "Global"),
                    ],
                    w,
                );
            });
            let i = view.selected_grade;
            let grade = if i == 3 {
                &mut r.effects.global_grade
            } else {
                &mut r.grading[i]
            };
            ui.push_id(("grade", i), |ui| {
                let mut degrees = grade[0] * 360.;
                slider(ui, "Hue", &mut degrees, 0. ..=360., 0.);
                if degrees != grade[0] * 360. {
                    grade[0] = degrees / 360.;
                }
                let tint = crate::develop::color::hue_rgb(grade[0]).map(|v| (v * 180.) as u8);
                slider_with(
                    ui,
                    "Saturation",
                    &mut grade[1],
                    0. ..=1.,
                    0.,
                    None,
                    Some((
                        theme::gray(90),
                        Color32::from_rgb(tint[0], tint[1], tint[2]),
                    )),
                );
                slider_with(
                    ui,
                    "Luminance",
                    &mut grade[2],
                    -1. ..=1.,
                    0.,
                    None,
                    Some((theme::gray(25), theme::gray(210))),
                );
            });
            ui.add_space(4.);
            slider(ui, "Blending", &mut r.effects.blending, 0. ..=1., 0.5);
            slider(ui, "Balance", &mut r.effects.balance, -1. ..=1., 0.);
        }) {
            r.grading = [[0.; 3]; 3];
            r.effects.global_grade = [0.; 3];
            r.effects.balance = 0.;
            r.effects.blending = 0.5;
        }

        if adjustment_section(ui, "Detail", |ui| {
            subheading(ui, "Sharpening");
            slider_with(
                ui,
                "Amount",
                &mut r.sharpening,
                0. ..=1.,
                0.35,
                Some((150., 0)),
                None,
            );
            if r.engine >= 3 {
                slider_with(
                    ui,
                    "Radius",
                    &mut r.sharpening_radius,
                    0.5..=3.,
                    0.8,
                    Some((1., 1)),
                    None,
                );
                slider(ui, "Detail", &mut r.sharpening_detail, 0. ..=1., 0.25);
                slider(ui, "Masking", &mut r.sharpening_masking, 0. ..=1., 0.35);
            }
            subheading(ui, "Noise Reduction");
            slider(ui, "Luminance", &mut r.noise_luma, 0. ..=1., 0.);
            ui.push_id("luma-nr", |ui| {
                slider(ui, "Detail", &mut r.effects.luma_detail, 0. ..=1., 0.5);
                slider(ui, "Contrast", &mut r.effects.luma_contrast, 0. ..=1., 0.);
            });
            slider(ui, "Color", &mut r.noise_chroma, 0. ..=1., 0.);
            ui.push_id("color-nr", |ui| {
                slider(ui, "Detail", &mut r.effects.chroma_detail, 0. ..=1., 0.5);
                slider(
                    ui,
                    "Smoothness",
                    &mut r.effects.chroma_smoothness,
                    0. ..=1.,
                    0.5,
                );
            });
        }) {
            let d = Recipe::default().effects;
            r.noise_luma = 0.;
            r.noise_chroma = 0.;
            r.effects.luma_detail = d.luma_detail;
            r.effects.luma_contrast = d.luma_contrast;
            r.effects.chroma_detail = d.chroma_detail;
            r.effects.chroma_smoothness = d.chroma_smoothness;
            r.sharpening = if r.engine >= 3 { 0.35 } else { 0. };
            r.sharpening_radius = 0.8;
            r.sharpening_detail = 0.25;
            r.sharpening_masking = 0.35;
        }

        if adjustment_section(ui, "Lens Corrections", |ui| {
            subheading(ui, "Profile");
            let builtin = metadata.as_ref().and_then(|m| m.lens.as_ref());
            let adobe = metadata.as_ref().and_then(|m| m.profile_lens.as_ref());
            ui.add_enabled_ui(r.engine >= 4, |ui| {
                control_row(ui, "", |ui| {
                    if ui
                        .checkbox(&mut r.lens_profile, "Enable Profile Corrections")
                        .on_hover_text("Correct distortion and vignetting with an Adobe lens profile, or the lens data the camera stored in the RAW.")
                        .on_disabled_hover_text("Update the process version in Calibration to use lens corrections.")
                        .changed()
                        // Sony's built-in data is off by default, so it follows the
                        // checkbox; Fuji and DNG built-ins stay on, as in Lightroom.
                        && builtin.is_some_and(|l| !l.default_on)
                    {
                        r.lens_builtin = r.lens_profile;
                    }
                });
            });
            let lens = metadata
                .as_ref()
                .map(|m| m.lens_model.clone())
                .filter(|l| !l.is_empty());
            control_row(ui, "Lens", |ui| {
                ui.label(
                    egui::RichText::new(lens.as_deref().unwrap_or("Unknown"))
                        .size(11.)
                        .color(theme::gray(200)),
                );
            });
            control_row(ui, "Profile", |ui| {
                ui.label(
                    egui::RichText::new(
                        adobe
                            .or(builtin)
                            .map_or("No matching profile", |l| l.source.as_str()),
                    )
                    .size(11.)
                    .color(theme::gray(200)),
                );
            });
            if r.lens_profile {
                subheading(ui, "Amount");
                ui.push_id("lens-amount", |ui| {
                    slider_with(
                        ui,
                        "Distortion",
                        &mut r.lens_distortion,
                        0. ..=2.,
                        1.,
                        Some((100., 0)),
                        None,
                    );
                    slider_with(
                        ui,
                        "Vignetting",
                        &mut r.lens_vignetting,
                        0. ..=2.,
                        1.,
                        Some((100., 0)),
                        None,
                    );
                });
            }
            control_row(ui, "", |ui| {
                if ui
                    .small_button("Import Lens Profiles…")
                    .on_hover_text("Import Adobe .lcp lens profiles, e.g. from /Library/Application Support/Adobe/CameraRaw/LensProfiles")
                    .clicked()
                {
                    import_lens = true;
                }
                if ui
                    .small_button("Import Folder…")
                    .on_hover_text("Import every .lcp lens profile in a folder and its subfolders")
                    .clicked()
                {
                    import_folder = Some(ImportKind::LensProfiles);
                }
            });
            subheading(ui, "Defringe");
            for (i, name, hue) in [(0, "Purple", [0.55, 0.9]), (1, "Green", [0.2, 0.5])] {
                ui.push_id(("defringe", i), |ui| {
                    slider_with(
                        ui,
                        &format!("{name} Amount"),
                        &mut r.effects.defringe[i],
                        0. ..=1.,
                        0.,
                        Some((20., 0)),
                        None,
                    );
                    let [lo, hi] = &mut r.effects.defringe_ranges[i];
                    let colors = hue.map(|h| {
                        let c = crate::develop::color::hue_rgb(h).map(|v| (v * 180.) as u8);
                        Color32::from_rgb(c[0], c[1], c[2])
                    });
                    slider_with(
                        ui,
                        &format!("{name} Hue"),
                        lo,
                        0. ..=(*hi - 0.1).max(0.),
                        0.3,
                        None,
                        Some((colors[0], colors[1])),
                    );
                    ui.push_id("hi", |ui| {
                        slider_with(
                            ui,
                            "",
                            hi,
                            (*lo + 0.1).min(1.)..=1.,
                            0.7,
                            None,
                            Some((colors[0], colors[1])),
                        );
                    });
                });
            }
            subheading(ui, "Vignetting");
            ui.push_id("lens-vignette", |ui| {
                slider(ui, "Amount", &mut r.effects.lens_vignette, -1. ..=1., 0.);
                slider(
                    ui,
                    "Midpoint",
                    &mut r.effects.lens_vignette_midpoint,
                    0. ..=1.,
                    0.5,
                );
            });
        }) {
            if let Some(m) = &metadata {
                r.lens_builtin = m.lens.as_ref().is_none_or(|l| l.default_on);
            }
            let defaults = Recipe::default();
            r.lens_profile = defaults.lens_profile;
            r.lens_distortion = defaults.lens_distortion;
            r.lens_vignetting = defaults.lens_vignetting;
            let d = Recipe::default().effects;
            r.effects.defringe = d.defringe;
            r.effects.defringe_ranges = d.defringe_ranges;
            r.effects.lens_vignette = d.lens_vignette;
            r.effects.lens_vignette_midpoint = d.lens_vignette_midpoint;
        }

        if adjustment_section(ui, "Transform", |ui| {
            let supported = r.engine >= 4;
            if !supported {
                hint_row(ui, "Update the process in Calibration to use Transform.");
            }
            ui.add_enabled_ui(supported, |ui| {
                control_row(ui, "Upright", |ui| {
                    // Automatic Upright needs line detection, which isn't built yet.
                    ui.add_enabled_ui(false, |ui| {
                        let w = ui.available_width();
                        let mut mode = 0;
                        segmented(
                            ui,
                            &mut mode,
                            &[(0, "Off"), (1, "Auto"), (2, "Level"), (3, "Vertical"), (4, "Full")],
                            w,
                        );
                    })
                    .response
                    .on_disabled_hover_text("Automatic Upright isn't available yet; use the sliders.");
                });
                control_row(ui, "", |ui| {
                    let mut constrain = false;
                    ui.add_enabled(false, egui::Checkbox::new(&mut constrain, "Constrain Crop"))
                        .on_disabled_hover_text("Not available yet. Areas outside the photo render white; crop them out with the Crop tool.");
                });
                subheading(ui, "Transform");
                let t = &mut r.transform;
                slider(ui, "Vertical", &mut t.vertical, -1. ..=1., 0.);
                slider(ui, "Horizontal", &mut t.horizontal, -1. ..=1., 0.);
                slider_with(ui, "Rotate", &mut t.rotate, -10. ..=10., 0., Some((1., 1)), None);
                slider(ui, "Aspect", &mut t.aspect, -1. ..=1., 0.);
                slider_with(ui, "Scale", &mut t.scale, 0.5..=1.5, 1., Some((100., 0)), None);
                slider(ui, "Offset X", &mut t.offset_x, -1. ..=1., 0.);
                slider(ui, "Offset Y", &mut t.offset_y, -1. ..=1., 0.);
            });
        }) {
            r.transform = Default::default();
        }

        if adjustment_section(ui, "Effects", |ui| {
            subheading(ui, "Post-Crop Vignetting");
            slider(ui, "Amount", &mut r.effects.vignette, -1. ..=1., 0.);
            slider(
                ui,
                "Midpoint",
                &mut r.effects.vignette_midpoint,
                0. ..=1.,
                0.5,
            );
            slider(
                ui,
                "Feather",
                &mut r.effects.vignette_feather,
                0. ..=1.,
                0.5,
            );
            subheading(ui, "Grain");
            ui.push_id("grain", |ui| {
                slider(ui, "Amount", &mut r.effects.grain, 0. ..=1., 0.);
                slider(ui, "Size", &mut r.effects.grain_size, 0. ..=1., 0.25);
                slider(
                    ui,
                    "Roughness",
                    &mut r.effects.grain_roughness,
                    0. ..=1.,
                    0.5,
                );
            });
        }) {
            r.effects.grain = 0.;
            r.effects.grain_size = 0.25;
            r.effects.grain_roughness = 0.5;
            r.effects.vignette_midpoint = 0.5;
            r.effects.vignette_feather = 0.5;
            r.effects.vignette = 0.;
        }

        if adjustment_section(ui, "Calibration", |ui| {
            subheading(ui, "Process");
            if r.engine < 4 {
                ui.horizontal(|ui| {
                    ui.small(format!("Version {} (older rendering preserved)", r.engine));
                    if ui
                        .small_button("Update")
                        .on_hover_text(
                            "Render with the current process, including built-in lens corrections",
                        )
                        .clicked()
                    {
                        if r.engine < 3 {
                            r.profile = metadata.as_ref().and_then(crate::camera_profiles::builtin);
                            if r.sharpening == 0. {
                                r.sharpening = 0.35;
                            }
                        }
                        r.engine = 4;
                    }
                });
            } else {
                ui.small("Current version");
            }
            if r.profile.is_some() {
                ui.checkbox(&mut r.profile_tone, "Profile tone rendering")
                    .on_hover_text("Use the camera profile's base tone curve. Older edits retain their original rendering until enabled.");
                if let Some(m) = &metadata {
                    let baseline = crate::camera_profiles::reference::baseline_exposure(m);
                    if baseline != r.camera_exposure
                        && ui.small_button("Use camera exposure baseline")
                            .on_hover_text("Apply the camera's reference exposure offset while leaving the Exposure slider unchanged.")
                            .clicked()
                    {
                        r.camera_exposure = baseline;
                    }
                }
            }
            if ui.checkbox(&mut r.reference_curves, "Reference tone curves")
                .on_hover_text("Natural cubic curves, hue-preserving master processing and an sRGB transfer in ProPhoto RGB. Older edits keep their saved curve behavior until enabled.")
                .changed()
                && r.reference_curves
            {
                r.curve.natural = true;
                r.curve.smooth = true;
                for curve in &mut r.effects.channels {
                    curve.natural = true;
                    curve.smooth = true;
                }
            }
            if !r.reference_curves {
                ui.checkbox(&mut r.wide_gamut_curves, "Legacy wide-gamut curves");
            }
            ui.checkbox(&mut r.reference_color, "Reference color rendering")
                .on_hover_text("Updated vibrance, color mixer luminance and RGB-hue split toning. Older saved edits retain their original rendering until enabled.");
            ui.checkbox(&mut r.reference_calibration, "Reference calibration")
                .on_hover_text("Neutral-preserving primary adjustments and corrected shadow tint. Older edits keep their saved behavior until enabled.");
            subheading(ui, "Shadows");
            ui.push_id("calibration-shadows", |ui| {
                slider_with(
                    ui,
                    "Tint",
                    &mut r.effects.shadow_tint,
                    -1. ..=1.,
                    0.,
                    None,
                    Some((
                        Color32::from_rgb(91, 156, 112),
                        Color32::from_rgb(165, 105, 158),
                    )),
                );
            });
            for (i, name) in ["Red Primary", "Green Primary", "Blue Primary"]
                .iter()
                .enumerate()
            {
                subheading(ui, name);
                ui.push_id(("calibration", i), |ui| {
                    slider(ui, "Hue", &mut r.effects.calibration[i][0], -1. ..=1., 0.);
                    slider(
                        ui,
                        "Saturation",
                        &mut r.effects.calibration[i][1],
                        -1. ..=1.,
                        0.,
                    );
                });
            }
        }) {
            r.effects.calibration = [[0.; 2]; 3];
            r.effects.shadow_tint = 0.;
        }
        if let Some(kind) = auto_request {
            self.start_auto(kind);
        }
        if import_profiles {
            self.dialog(FileDialog::CameraProfile, &ui.ctx().clone());
        }
        if import_lens {
            self.dialog(FileDialog::LensProfile, &ui.ctx().clone());
        }
        if let Some(kind) = import_folder {
            self.dialog(FileDialog::ImportFolder(kind), &ui.ctx().clone());
        }
        if import_adobe && let Some(m) = self.document.metadata.clone() {
            if let Some(key) = adobe_key {
                ui.ctx()
                    .data_mut(|d| d.remove::<Vec<std::path::PathBuf>>(key));
            }
            match crate::camera_profiles::import_files(&adobe) {
                Ok(done) => {
                    let (profiles, errors) = crate::camera_profiles::installed(&m);
                    self.document.profiles = profiles;
                    self.document.profile_errors = errors;
                    self.refresh_preset_support();
                    self.status = format!(
                        "Imported {} Adobe profiles for {} {}. Choose one from the Profile menu.",
                        done.len(),
                        m.make,
                        m.model
                    );
                }
                Err(e) => self.status = format!("Profiles not imported: {e:#}"),
            }
        }
    }
}

const BANDS: [&str; 8] = [
    "Red", "Orange", "Yellow", "Green", "Aqua", "Blue", "Purple", "Magenta",
];
fn band_color(i: usize) -> Color32 {
    [
        Color32::from_rgb(208, 69, 61),
        Color32::from_rgb(225, 137, 55),
        Color32::from_rgb(213, 193, 61),
        Color32::from_rgb(90, 160, 92),
        Color32::from_rgb(72, 178, 178),
        Color32::from_rgb(78, 125, 207),
        Color32::from_rgb(141, 103, 191),
        Color32::from_rgb(193, 92, 165),
    ][i]
}
/// Rail colors for a mixer slider: neighbouring hues for Hue, gray to color
/// for Saturation and dark to light for Luminance.
fn hsl_gradient(band: usize, channel: usize) -> (Color32, Color32) {
    let color = band_color(band);
    match channel {
        0 => (band_color((band + 7) % 8), band_color((band + 1) % 8)),
        1 => (theme::gray(110), color),
        _ => (theme::gray(25), color.lerp_to_gamma(Color32::WHITE, 0.45)),
    }
}
/// Group caption (Tone, Presence…) starting where the slider rails start.
fn subheading(ui: &mut egui::Ui, text: &str) {
    super::widgets::set_edit_context(ui, text);
    ui.add_space(8.);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 16.), Sense::hover());
    ui.painter().text(
        rect.left_center() + Vec2::new(88., 0.),
        egui::Align2::LEFT_CENTER,
        text,
        egui::FontId::proportional(11.),
        theme::gray(165),
    );
}
/// A labelled control row on the slider grid: caption right-aligned in the
/// 83 px label column, controls from the rail start (88 px) to the right edge,
/// and the same height as a slider row.
fn control_row<R>(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let (row, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 26.), Sense::hover());
    ui.painter().text(
        Pos2::new(row.left() + 78., row.center().y),
        egui::Align2::RIGHT_CENTER,
        label,
        egui::FontId::proportional(11.),
        theme::gray(190),
    );
    let rect = Rect::from_min_max(Pos2::new(row.left() + 88., row.top()), row.max);
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
        |ui| {
            ui.spacing_mut().item_spacing.x = 4.;
            add(ui)
        },
    )
    .inner
}
/// Lightroom's white balance presets for RAW files: (name, kelvin, tint).
const WB_PRESETS: [(&str, f32, f32); 6] = [
    ("Daylight", 5500., 10.),
    ("Cloudy", 6500., 10.),
    ("Shade", 7500., 10.),
    ("Tungsten", 2850., 0.),
    ("Fluorescent", 3800., 21.),
    ("Flash", 5500., 0.),
];
fn eyedropper_icon(painter: &egui::Painter, c: Pos2, strong: bool) {
    let color = theme::gray(if strong { 235 } else { 170 });
    icons::paint_at(painter, Icon::Eyedropper, c, 14., color);
}
/// The line that opens the Remove and Masking drawers: both tools are new.
fn experimental(ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.add_space(83.);
        ui.label(
            egui::RichText::new("Experimental · early version")
                .size(10.)
                .color(theme::warning()),
        )
        .on_hover_text(
            "Not yet measured against Lightroom. Spots and masks are saved apart from the \
             rest of the edit, so older RAWmakase releases open the photo without them.",
        );
    });
}
/// A circle with an arrow leaving it: the Remove tool.
fn heal_icon(painter: &egui::Painter, c: Pos2, strong: bool) {
    let stroke = Stroke::new(1.4, theme::gray(if strong { 240 } else { 170 }));
    painter.circle_stroke(c + Vec2::new(-2., 2.), 4.5, stroke);
    painter.line_segment([c + Vec2::new(1.5, -1.5), c + Vec2::new(6., -6.)], stroke);
    painter.line_segment([c + Vec2::new(6., -6.), c + Vec2::new(2.5, -6.)], stroke);
    painter.line_segment([c + Vec2::new(6., -6.), c + Vec2::new(6., -2.5)], stroke);
}
/// A dashed circle over a square: the Masking tool.
fn mask_icon(painter: &egui::Painter, c: Pos2, strong: bool) {
    let color = theme::gray(if strong { 240 } else { 170 });
    let stroke = Stroke::new(1.4, color);
    painter.rect_stroke(
        Rect::from_center_size(c, Vec2::splat(12.)),
        1.,
        stroke,
        egui::StrokeKind::Inside,
    );
    painter.circle_filled(c + Vec2::new(1., 1.), 3.5, color);
}
fn crop_icon(painter: &egui::Painter, c: Pos2, strong: bool) {
    let color = theme::gray(if strong { 240 } else { 170 });
    icons::paint_at(painter, Icon::Crop, c, 15., color);
}

/// Adobe's own profiles for this camera from a local Lightroom / Camera Raw
/// installation (Adobe Standard plus Camera Matching), skipping ones RAWmakase
/// already has.
fn adobe_camera_profiles(m: &crate::raw::Metadata) -> Vec<std::path::PathBuf> {
    let root = std::path::Path::new(if cfg!(target_os = "macos") {
        "/Library/Application Support/Adobe/CameraRaw/CameraProfiles"
    } else if cfg!(windows) {
        "C:\\ProgramData\\Adobe\\CameraRaw\\CameraProfiles"
    } else {
        return Vec::new();
    });
    let camera = format!("{} {}", m.make, m.model);
    let mut paths = vec![
        root.join("Adobe Standard")
            .join(format!("{camera} Adobe Standard.dcp")),
    ];
    if let Ok(entries) = std::fs::read_dir(root.join("Camera").join(&camera)) {
        paths.extend(entries.flatten().map(|e| e.path()));
    }
    let installed = crate::storage::data_dir().join("camera-profiles");
    paths
        .into_iter()
        .filter(|p| {
            p.is_file()
                && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("dcp"))
                && p.file_name().is_some_and(|n| !installed.join(n).exists())
        })
        .collect()
}
/// A small note aligned with the slider rails.
fn hint_row(ui: &mut egui::Ui, text: &str) {
    ui.horizontal(|ui| {
        ui.add_space(88.);
        ui.label(egui::RichText::new(text).size(11.).color(theme::gray(140)));
    });
}
