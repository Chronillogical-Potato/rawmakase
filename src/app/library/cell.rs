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
        if photo.master.is_some() {
            copy_badge(painter, image, fill);
        }
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
    if photo.master.is_some() {
        let name = if photo.copy_name.is_empty() {
            "Copy"
        } else {
            &photo.copy_name
        };
        let galley = painter.layout_no_wrap(name.into(), FontId::proportional(10.), ink);
        let space = cell.width() * 0.36;
        let left = cell.center().x - galley.size().x.min(space) / 2.;
        painter
            .with_clip_rect(Rect::from_min_size(
                Pos2::new(left, y - 7.),
                Vec2::new(space, 14.),
            ))
            .galley(Pos2::new(left, y - galley.size().y / 2.), galley, ink);
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
    let action = photo_menu(&response, photo);
    (
        response.on_hover_text(format!(
            "{}{}\n{}\n{}\n{}",
            photo.path.display(),
            copy_suffix(photo),
            photo.captured,
            photo.keywords,
            photo.label
        )),
        action,
    )
}
/// Lightroom's virtual copy badge: the image's lower left corner folded
/// over. `background` is what shows behind the fold.
pub(in crate::app) fn copy_badge(painter: &egui::Painter, image: egui::Rect, background: Color32) {
    let size = (image.width().min(image.height()) * 0.12).clamp(8., 16.);
    let corner = image.left_bottom();
    let up = corner - Vec2::new(0., size);
    let right = corner + Vec2::new(size, 0.);
    painter.add(egui::Shape::convex_polygon(
        vec![corner, right, up],
        background,
        egui::Stroke::NONE,
    ));
    painter.add(egui::Shape::convex_polygon(
        vec![up, right, corner + Vec2::new(size, -size)],
        theme::gray(225),
        egui::Stroke::new(1., Color32::from_black_alpha(160)),
    ));
}
/// " / Copy 1" for a virtual copy, as Lightroom names it after the file.
pub(in crate::app) fn copy_suffix(photo: &Photo) -> String {
    match photo.master {
        Some(_) if photo.copy_name.is_empty() => " / Copy".into(),
        Some(_) => format!(" / {}", photo.copy_name),
        None => String::new(),
    }
}
/// What a thumbnail's context menu asked for.
pub(in crate::app) enum PhotoAction {
    Develop,
    Reveal,
    CopyPath,
    Edit(crate::app::photo_metadata::Edit),
    Copy(super::CopyAction),
}
/// Create Virtual Copy's shortcut, as Lightroom shows it.
pub(in crate::app) const VIRTUAL_COPY_SHORTCUT: &str = if cfg!(target_os = "macos") {
    "⌘'"
} else {
    "Ctrl+'"
};
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
        use super::CopyAction;
        let copy = photo.master.is_some();
        for (title, shortcut, enabled, choice) in [
            (
                "Create Virtual Copy",
                VIRTUAL_COPY_SHORTCUT,
                true,
                CopyAction::Create(photo.id),
            ),
            (
                "Set Copy as Master",
                "",
                copy,
                CopyAction::SetMaster(photo.id),
            ),
            (
                "Remove Virtual Copy…",
                "",
                copy,
                CopyAction::Remove(photo.id),
            ),
        ] {
            if menu_item(ui, title, shortcut, enabled, false) {
                action = Some(PhotoAction::Copy(choice));
                ui.close();
            }
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
