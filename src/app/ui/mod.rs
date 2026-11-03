mod bottom;
mod central;
mod dialogs;
mod header;
mod settings;

use super::style::TEXT_BRIGHT;
use egui::{Color32, CursorIcon, RichText, TextureHandle, Ui, vec2};

/// Bouton icône ; si l'icône manque, un libellé texte le remplace.
pub(super) fn icon_button(
    ui: &mut Ui,
    icon: Option<&TextureHandle>,
    icon_size: f32,
    size: f32,
    fallback: &str,
    fill: Color32,
) -> egui::Response {
    let button = match icon {
        Some(texture) => egui::Button::image(
            egui::Image::from_texture(egui::load::SizedTexture::from_handle(texture))
                .fit_to_exact_size(vec2(icon_size, icon_size)),
        ),
        None => egui::Button::new(RichText::new(fallback).color(TEXT_BRIGHT)),
    };
    ui.add(button.fill(fill).min_size(vec2(size, size))).on_hover_cursor(CursorIcon::PointingHand)
}
