use crate::app::style::{TEXT_DIM, bold};
use crate::app::{REPOSITORY_URL, VideoCutApp};
use egui::{Align2, Context, RichText, vec2};

impl VideoCutApp {
    pub(in crate::app) fn settings(&mut self, ctx: &Context) {
        if !self.settings_open {
            return;
        }
        let mut open = true;
        let mut check_now = false;
        let mut choose = false;
        let mut reset_custom = false;
        egui::Window::new("Paramètres")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.set_width(460.0);
                ui.label(RichText::new("Composants externes").font(bold(15.0)));
                ui.add_space(4.0);
                ui.label(self.tools.describe());
                if let Some(path) = &self.tools.ffmpeg {
                    ui.label(RichText::new(path.display().to_string()).size(11.0).color(TEXT_DIM));
                }
                ui.add_space(6.0);
                let mut auto = self.config.auto_update;
                if ui.checkbox(&mut auto, "Mettre FFmpeg à jour automatiquement (vérification quotidienne)").changed()
                {
                    self.config.auto_update = auto;
                    self.save_config();
                }
                ui.horizontal(|ui| {
                    if ui.add_enabled(self.update.is_none(), egui::Button::new("Vérifier maintenant")).clicked() {
                        check_now = true;
                    }
                    if ui.button("Choisir un ffmpeg.exe…").clicked() {
                        choose = true;
                    }
                    if self.config.custom_ffmpeg.is_some() && ui.button("Revenir au FFmpeg automatique").clicked() {
                        reset_custom = true;
                    }
                });
                if let Some(status) = &self.update_status {
                    ui.label(RichText::new(status).size(12.0).color(TEXT_DIM));
                }
                ui.add_space(6.0);
                ui.label(
                    RichText::new(
                        "FFmpeg provient des versions officielles gyan.dev (GPL) ; chaque archive est vérifiée \
                         par empreinte SHA-256 avant installation.",
                    )
                    .size(11.0)
                    .color(TEXT_DIM),
                );
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(format!("VideoCut {}", env!("CARGO_PKG_VERSION")));
                    if ui.link("Page du projet sur GitHub").clicked() {
                        ctx.open_url(egui::OpenUrl::new_tab(REPOSITORY_URL));
                    }
                });
            });
        if check_now {
            self.start_update(ctx, true);
        }
        if choose {
            self.choose_custom_ffmpeg();
        }
        if reset_custom {
            self.config.custom_ffmpeg = None;
            self.save_config();
            self.relocate_tools();
        }
        self.settings_open = open;
    }
}
