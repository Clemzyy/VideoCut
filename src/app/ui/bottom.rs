use super::icon_button;
use crate::app::VideoCutApp;
use crate::app::style::{BG, ERROR, TEXT, TEXT_BRIGHT, TEXT_DIM, bold};
use crate::timefmt::format_time;
use crate::waveform::{self, Marker};
use egui::{Align, Context, FontId, Id, Layout, RichText, Ui, vec2};
use std::sync::atomic::Ordering;

impl VideoCutApp {
    pub(in crate::app) fn bottom(&mut self, ui: &mut Ui, ctx: &Context) {
        ui.add_space(6.0);
        self.progress_row(ui);
        ui.add_space(8.0);
        self.transport_row(ui, ctx);
        ui.add_space(4.0);
        self.selection_row(ui);
        ui.add_space(6.0);
        self.status_row(ui, ctx);
        ui.add_space(4.0);
    }

    fn progress_row(&self, ui: &mut Ui) {
        let ready = self.media.is_some();
        // Progression : analyse, export ou téléchargement de FFmpeg.
        let (fraction, label) = if let Some(export) = &self.export {
            (export.progress(), format!("Découpage : {:.0} %", export.progress() * 100.0))
        } else if let Some(task) = &self.loading {
            let ratio = task.progress.load(Ordering::Relaxed) as f32 / 1000.0;
            (ratio, format!("Analyse du fichier : {:.0} %", ratio * 100.0))
        } else if let Some(ratio) = self.download_progress {
            (ratio, format!("FFmpeg : {:.0} %", ratio * 100.0))
        } else if ready {
            (1.0, "Analyse terminée".to_string())
        } else {
            (0.0, String::new())
        };
        ui.horizontal(|ui| {
            let reserved = if self.export.is_some() { 300.0 } else { 200.0 };
            ui.add(
                egui::ProgressBar::new(fraction)
                    .desired_width((ui.available_width() - reserved).max(50.0))
                    .desired_height(10.0),
            );
            ui.label(RichText::new(label).color(TEXT));
            if let Some(export) = &self.export {
                if ui.button("Annuler").clicked() {
                    export.cancel();
                }
            }
        });
    }

    fn transport_row(&mut self, ui: &mut Ui, ctx: &Context) {
        let ready = self.media.is_some();
        ui.horizontal(|ui| {
            ui.set_min_height(60.0);
            let playing = self.playback.is_some();
            let play_label = if playing { "⏸  Pause" } else { "▶  Lecture" };
            if ui
                .add_enabled(ready, egui::Button::new(play_label).min_size(vec2(110.0, 34.0)))
                .on_hover_text("Espace")
                .clicked()
            {
                self.toggle_playback(ctx);
            }
            if ui
                .add_enabled(ready, egui::Button::new("⏹  Stop").min_size(vec2(90.0, 34.0)))
                .on_hover_text("Origine")
                .clicked()
            {
                self.stop();
            }
            ui.add_space(18.0);
            let volume_label = ui.label("Volume");
            let mut percent = self.config.volume_percent;
            let slider =
                ui.add(egui::Slider::new(&mut percent, 0..=200).show_value(false)).labelled_by(volume_label.id);
            ui.label(format!("{percent} %"));
            if slider.changed() {
                self.config.volume_percent = percent;
                self.volume.set_percent(percent);
            }
            if slider.drag_stopped() || slider.lost_focus() {
                self.save_config();
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let enabled = ready && self.export.is_none();
                let button = ui.add_enabled_ui(enabled, |ui| {
                    icon_button(ui, self.icons.export.as_ref(), 48.0, 58.0, "Exporter", BG)
                });
                if button.inner.on_hover_text("Ctrl+S").clicked() {
                    self.export_clip(ctx);
                }
                ui.label(RichText::new("Découpage et/ou conversion -->").font(bold(14.0)).color(TEXT_BRIGHT));
            });
        });
    }

    fn selection_row(&mut self, ui: &mut Ui) {
        let ready = self.media.is_some();
        ui.horizontal(|ui| {
            ui.add_enabled_ui(ready, |ui| {
                let start_label = ui.label(RichText::new("Début").color(waveform::START_COLOR));
                self.time_field(ui, Marker::Start, start_label.id);
                ui.add_space(10.0);
                let end_label = ui.label(RichText::new("Fin").color(waveform::END_COLOR));
                self.time_field(ui, Marker::End, end_label.id);
                ui.add_space(10.0);
                ui.label("Durée extrait");
                let clip = if ready { self.view.end - self.view.start } else { 0.0 };
                ui.label(RichText::new(format_time(clip)).font(bold(14.0)));
            })
            .response
            .on_hover_text("Raccourcis : I place le début, O place la fin à la tête de lecture.");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let mut precise = self.config.precise_export;
                let combo = egui::ComboBox::from_id_salt("export-mode")
                    .width(250.0)
                    .selected_text(if precise { "Précise (réencodage)" } else { "Rapide (sans réencodage)" })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut precise, true, "Précise (réencodage)")
                            .on_hover_text("Coupe à l'image près, comme dans l'aperçu. Plus lent.");
                        ui.selectable_value(&mut precise, false, "Rapide (sans réencodage)")
                            .on_hover_text("Aucune perte de qualité, quasi instantané, mais le début d'une vidéo se cale sur l'image-clé précédente.");
                    });
                if precise != self.config.precise_export {
                    self.config.precise_export = precise;
                    self.save_config();
                }
                let mode_label = ui.label("Coupe :");
                combo.response.labelled_by(mode_label.id);
            });
        });
    }

    fn status_row(&mut self, ui: &mut Ui, ctx: &Context) {
        ui.horizontal(|ui| {
            let status_color = if self.status.starts_with("Erreur") { ERROR } else { TEXT };
            let tools_label = format!("⚙ {}", self.tools.describe());
            let right_width = 260.0 + if self.app_update.is_some() { 230.0 } else { 0.0 };
            let status_size = vec2((ui.available_width() - right_width).max(80.0), 18.0);
            ui.allocate_ui_with_layout(status_size, Layout::left_to_right(Align::Center), |ui| {
                ui.add(egui::Label::new(RichText::new(&self.status).color(status_color)).truncate());
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let tools_color = if self.tools.ffmpeg.is_some() { TEXT_DIM } else { ERROR };
                let settings =
                    ui.add(egui::Button::new(RichText::new(tools_label).size(12.0).color(tools_color)).frame(false));
                if settings
                    .on_hover_text(self.update_status.clone().unwrap_or_else(|| "Paramètres et mises à jour".into()))
                    .clicked()
                {
                    self.settings_open = true;
                }
                if let Some((version, url)) = &self.app_update {
                    if ui.link(format!("Nouvelle version {version} disponible")).clicked() {
                        ctx.open_url(egui::OpenUrl::new_tab(url));
                    }
                }
            });
        });
    }

    fn time_field(&mut self, ui: &mut Ui, marker: Marker, label: Id) {
        let text = if marker == Marker::Start { &mut self.start_text } else { &mut self.end_text };
        let response = ui
            .add(egui::TextEdit::singleline(text).desired_width(96.0).font(FontId::monospace(14.0)))
            .labelled_by(label);
        if response.lost_focus() {
            self.apply_time_field(marker);
        }
    }
}
