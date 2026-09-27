//! The interface's icons: Lucide outlines (fastframe-icons), drawn in white
//! and tinted where they are painted. RAWmakase's own copies live in
//! `assets/icons`; the rest come from fastframe's shared set.
use eframe::egui::{self, Color32, Rect};

fastframe_icons::icons! {
    /// Every icon the interface draws.
    pub(super) enum Icon {
        prefix: "rawmakase-icon-",
        directory: "../../assets/icons/",
        Undo => "undo-2",
        Redo => "redo-2",
        BeforeAfter => "columns-2",
        Export => "download",
        Reset => "rotate-ccw",
        Eyedropper => "pipette",
        Crop => "crop",
        Flag => "flag",
        Rejected => "flag-off",
        Folder => "folder",
        Settings => lucide "settings",
        Close => lucide "x",
        Check => lucide "check",
        Add => lucide "plus",
        More => lucide "ellipsis",
        ChevronDown => lucide "chevron-down",
        ChevronRight => lucide "chevron-right",
        Eye => lucide "eye",
        EyeOff => lucide "eye-off",
    }
}

/// Registers the icons with egui, after its SVG loader.
pub(super) fn install(ctx: &egui::Context) {
    egui_extras::install_image_loaders(ctx);
    fastframe_icons::install::<Icon>(ctx);
}

/// Paints `icon` into `rect`, tinted. Nothing is drawn until the SVG is
/// rasterized, which happens on first use at each size.
pub(super) fn paint(painter: &egui::Painter, icon: Icon, rect: Rect, tint: Color32) {
    let ctx = painter.ctx();
    let hint = egui::SizeHint::Height((rect.height() * ctx.pixels_per_point()).round() as u32);
    if let Ok(egui::load::TexturePoll::Ready { texture }) =
        ctx.try_load_texture(icon.uri(), egui::TextureOptions::LINEAR, hint)
    {
        let uv = Rect::from_min_max(egui::pos2(0., 0.), egui::pos2(1., 1.));
        painter.image(texture.id, rect, uv, tint);
    }
}

/// Paints `icon` `size` points square, centred on `center`.
pub(super) fn paint_at(
    painter: &egui::Painter,
    icon: Icon,
    center: egui::Pos2,
    size: f32,
    tint: Color32,
) {
    paint(
        painter,
        icon,
        Rect::from_center_size(center, egui::Vec2::splat(size)),
        tint,
    );
}
