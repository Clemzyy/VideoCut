use super::icon_button;
use crate::app::style::{BORDER, BUTTON_BG, PANEL, TEXT_BRIGHT, TEXT_DIM, bold};
use crate::app::{VideoCutApp, file_name};
use crate::assets::LINKS;
use egui::{Align, Context, Frame, Layout, Margin, RichText, Stroke, Ui, vec2};
use std::path::Path;

impl VideoCutApp {
    pub(in crate::app) fn header(&mut self, ui: &mut Ui, ctx: &Context) {
        Frame::NONE.fill(PANEL).stroke(Stroke::new(1.0, BORDER)).inner_margin(Margin::symmetric(14, 7)).show(
            ui,
            |ui| {
                ui.set_height(74.0);
                ui.horizontal_centered(|ui| {
                    let select = icon_button(ui, self.icons.select.as_ref(), 64.0, 72.0, "Ouvrir", BUTTON_BG)
                        .on_hover_text("Choisir un fichier audio ou vidéo (Ctrl+O), ou le glisser dans la fenêtre");
                    if select.clicked() {
                        self.choose_file(ctx);
                    }
                    ui.add_space(14.0);
                    let links_width = LINKS.len() as f32 * 44.0 + 10.0;
                    let label_width = (ui.available_width() - links_width).max(80.0);
                    ui.allocate_ui_with_layout(vec2(label_width, 72.0), Layout::top_down(Align::Min), |ui| {
                        ui.add_space(if self.media.is_some() { 14.0 } else { 24.0 });
                        let (title, subtitle) = match (&self.media, &self.loading) {
                            (Some(media), _) => (file_name(&media.path), parent_name(&media.path)),
                            (None, Some(task)) => (file_name(&task.path), Some("Analyse en cours…".into())),
                            (None, None) => ("<-- Sélectionner un fichier".to_string(), None),
                        };
                        ui.add(egui::Label::new(RichText::new(title).font(bold(18.0)).color(TEXT_BRIGHT)).truncate());
                        if let Some(subtitle) = subtitle {
                            ui.add(egui::Label::new(RichText::new(subtitle).size(12.0).color(TEXT_DIM)).truncate());
                        }
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        for (link, icon) in LINKS.iter().zip(&self.icons.links).rev() {
                            let response = icon_button(ui, icon.as_ref(), 32.0, 38.0, link.fallback, PANEL)
                                .on_hover_text(link.label);
                            if response.clicked() {
                                ctx.open_url(egui::OpenUrl::new_tab(link.url));
                            }
                        }
                    });
                });
            },
        );
    }
}

fn parent_name(path: &Path) -> Option<String> {
    path.parent().map(|p| p.display().to_string())
}
