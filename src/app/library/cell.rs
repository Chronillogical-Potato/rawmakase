use crate::app::theme;
use crate::catalog::Photo;
use eframe::egui::{self, Color32, Vec2};
pub(super) fn photo_cell(
    ui: &mut egui::Ui,
    photo: &Photo,
    texture: Option<&egui::TextureHandle>,
    selected: bool,
    number: usize,
    available: bool,
    width: f32,
) -> (egui::Response, Option<PhotoAction>) {
    use crate::app::photo_metadata::{flag_icon, label_color};
    use egui::{Align2, FontId, Pos2, Rect, Sense, Stroke, StrokeKind};
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(width), Sense::hover());
    // Selection reveals metadata in the side panel. A stable ID keeps the
    // first right-click menu open when that changes the surrounding widget tree.
    let response = ui.interact(
        rect,
        egui::Id::new(("catalog-photo-cell", photo.id)),
        Sense::click(),
    );
    // Lightroom grid: dark cells separated by thin gutters, a light surround
    // for the selection, and the color label tinting the cell.
    let cell = rect.shrink(1.);
    let base = if selected {
        theme::gray(150)
    } else if response.hovered() {
        theme::gray(66)
    } else {
        theme::gray(56)
    };
    let fill = label_color(&photo.label).map_or(base, |label| {
        base.lerp_to_gamma(label, if selected { 0.35 } else { 0.22 })
    });
    let painter = ui.painter();
    painter.rect_filled(cell, 1., fill);
    painter.rect_stroke(
        cell,
        1.,
        Stroke::new(1., theme::gray(if selected { 205 } else { 40 })),
        StrokeKind::Inside,
    );
    let ink = theme::gray(if selected { 60 } else { 125 });
    let header = (width * 0.13).clamp(16., 26.);
    painter.text(
        cell.left_top() + Vec2::new(6., 3.),
        Align2::LEFT_TOP,
        number.to_string(),
        FontId::proportional(header * 0.95),
        theme::gray(if selected { 120 } else { 78 }),
    );
    if width >= 140. {
        let name = painter.layout_no_wrap(photo.filename.clone(), FontId::proportional(10.), ink);
        let space = (cell.width() * 0.62).min(name.size().x);
        painter
            .with_clip_rect(Rect::from_min_size(
                Pos2::new(cell.right() - 7. - space, cell.top() + 5.),
                Vec2::new(space, 14.),
            ))
            .galley(
                Pos2::new(cell.right() - 7. - space, cell.top() + 5.),
                name,
                ink,
            );
    }
    let footer = 20.;
    let margin = (width * 0.08).max(8.);
    let area = Rect::from_min_max(
        Pos2::new(cell.left() + margin, cell.top() + header + 4.),
        Pos2::new(cell.right() - margin, cell.bottom() - footer - 2.),
    );
    if let Some(texture) = texture {
        let size = texture.size_vec2();
        let scale = (area.width() / size.x).min(area.height() / size.y);
        let image = Rect::from_center_size(area.center(), size * scale);
        painter.rect_filled(
            image.expand(1.).translate(Vec2::new(1.5, 2.)),
            0.,
            Color32::from_black_alpha(90),
        );
        painter.image(
            texture.id(),
            image,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1., 1.)),
            Color32::WHITE,
        );
        painter.rect_stroke(
            image,
            0.,
            Stroke::new(1., Color32::from_black_alpha(160)),
            StrokeKind::Outside,
        );
    } else {
        painter.text(
            area.center(),
            Align2::CENTER_CENTER,
            if available { &photo.format } else { "Offline" },
            FontId::proportional(11.),
            ink,
        );
    }
    let y = cell.bottom() - footer / 2. - 1.;
    let mut x = cell.left() + 7.;
    if photo.flag != 0 {
        flag_icon(painter, Pos2::new(x + 4., y), photo.flag, selected);
        x += 14.;
    }
    if photo.rating > 0 {
        painter.text(
            Pos2::new(x, y),
            Align2::LEFT_CENTER,
            "★".repeat(photo.rating as usize),
            FontId::proportional(10.),
            theme::gray(if selected { 35 } else { 185 }),
        );
    }
    if !photo.copy_name.is_empty() {
        painter.text(
            Pos2::new(cell.center().x, y),
            Align2::CENTER_CENTER,
            "Copy",
            FontId::proportional(9.),
            ink,
        );
    }
    if !available {
        painter.text(
            Pos2::new(cell.right() - 24., y),
            Align2::RIGHT_CENTER,
            "?",
            FontId::proportional(11.),
            Color32::from_rgb(210, 150, 60),
        );
    }
    if let Some(color) = label_color(&photo.label) {
        let badge = Rect::from_center_size(Pos2::new(cell.right() - 12., y), Vec2::splat(9.));
        painter.rect_filled(badge, 1., color);
        painter.rect_stroke(
            badge,
            1.,
            Stroke::new(1., theme::gray(30)),
            StrokeKind::Inside,
        );
    }
    let action = photo_menu(&response, photo);
    (
        response.on_hover_text(format!(
            "{}\n{}\n{}\n{}",
            photo.path.display(),
            photo.captured,
            photo.keywords,
            photo.label
        )),
        action,
    )
}
/// What a thumbnail's context menu asked for.
pub(in crate::app) enum PhotoAction {
    Develop,
    Reveal,
    CopyPath,
    Edit(crate::app::photo_metadata::Edit),
}
/// The right-click menu shared by grid cells and the Develop filmstrip.
pub(in crate::app) fn photo_menu(response: &egui::Response, photo: &Photo) -> Option<PhotoAction> {
    use crate::app::photo_metadata::{Edit, LABELS};
    use crate::app::widgets::{menu_item, menu_separator, submenu_style};
    let mut action = None;
    crate::app::widgets::context_menu(response, |ui| {
        ui.set_width(210.);
        ui.spacing_mut().item_spacing.y = 0.;
        if menu_item(ui, "Open in Develop", "D", true, false) {
            action = Some(PhotoAction::Develop);
            ui.close();
        }
        if menu_item(ui, crate::platform::reveal::LABEL, "", true, false) {
            action = Some(PhotoAction::Reveal);
            ui.close();
        }
        if menu_item(ui, "Copy File Path", "", true, false) {
            action = Some(PhotoAction::CopyPath);
            ui.close();
        }
        menu_separator(ui);
        submenu_style(ui);
        ui.menu_button("Set Flag", |ui| {
            ui.set_width(170.);
            ui.spacing_mut().item_spacing.y = 0.;
            for (flag, title) in [(1, "Flagged"), (0, "Unflagged"), (-1, "Rejected")] {
                if menu_item(
                    ui,
                    title,
                    ["X", "U", "P"][(flag + 1) as usize],
                    true,
                    photo.flag == flag,
                ) {
                    action = Some(PhotoAction::Edit(Edit::Flag(flag)));
                    ui.close();
                }
            }
        });
        ui.menu_button("Set Rating", |ui| {
            ui.set_width(170.);
            ui.spacing_mut().item_spacing.y = 0.;
            for rating in 0..=5 {
                let title = if rating == 0 {
                    "None".into()
                } else {
                    "★".repeat(rating as usize)
                };
                if menu_item(
                    ui,
                    &title,
                    &rating.to_string(),
                    true,
                    photo.rating == rating,
                ) {
                    action = Some(PhotoAction::Edit(Edit::Rating(rating)));
                    ui.close();
                }
            }
        });
        ui.menu_button("Set Color Label", |ui| {
            ui.set_width(170.);
            ui.spacing_mut().item_spacing.y = 0.;
            for (label, key) in LABELS
                .into_iter()
                .zip(["6", "7", "8", "9", ""])
                .chain([("", "")])
            {
                let title = if label.is_empty() { "None" } else { label };
                let before = ui.cursor().min;
                if menu_item(ui, title, key, true, photo.label == label) {
                    action = Some(PhotoAction::Edit(Edit::Label(label.into())));
                    ui.close();
                }
                if let Some(color) = crate::app::photo_metadata::label_color(label) {
                    let chip = egui::Rect::from_center_size(
                        before + Vec2::new(ui.available_width() - 60., 12.),
                        Vec2::splat(9.),
                    );
                    ui.painter().rect_filled(chip, 2., color);
                }
            }
        });
    });
    action
}
