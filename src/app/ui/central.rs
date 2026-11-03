const HANDLE_HEIGHT: f32 = 18.0;
const MIN_WAVE_HEIGHT: f32 = 150.0;
const MIN_VIDEO_HEIGHT: f32 = 120.0;

use crate::app::VideoCutApp;
use crate::app::style::{BORDER, HANDLE_BG, HANDLE_LINE, TEXT, TEXT_BRIGHT, TEXT_DIM, VIDEO_BG, bold};
use crate::waveform;
use egui::{Align2, Color32, Context, CursorIcon, FontId, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use std::sync::atomic::Ordering;

impl VideoCutApp {
    pub(in crate::app) fn central(&mut self, ui: &mut Ui, ctx: &Context) {
        let is_video = self.media.as_ref().is_some_and(|m| m.info.is_video());
        let available = ui.available_height();
        let video_height = if is_video {
            let max = (available - MIN_WAVE_HEIGHT - HANDLE_HEIGHT).max(MIN_VIDEO_HEIGHT);
            self.config.video_height.clamp(MIN_VIDEO_HEIGHT, max)
        } else {
            0.0
        };
        let wave_height = if is_video { available - video_height - HANDLE_HEIGHT } else { available };

        let loading_text;
        let placeholder = match (&self.media, &self.loading) {
            (Some(_), _) => None,
            (None, Some(task)) => {
                let percent = task.progress.load(Ordering::Relaxed) as f32 / 10.0;
                loading_text = format!("Analyse en cours… {percent:.0} %");
                Some(loading_text.as_str())
            }
            (None, None) => Some("Clique sur l'icône en haut à gauche ou glisse un fichier audio/vidéo ici"),
        };
        let peaks = self.media.as_ref().and_then(|m| m.peaks.as_ref());
        let view = &mut self.view;
        let events =
            ui.allocate_ui(vec2(ui.available_width(), wave_height), |ui| view.show(ui, peaks, placeholder)).inner;
        for event in events {
            self.on_wave_event(event, ctx);
        }
        if is_video {
            self.resize_handle(ui, available);
            self.video_panel(ui, video_height);
        }
    }

    fn resize_handle(&mut self, ui: &mut Ui, available: f32) {
        let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), HANDLE_HEIGHT), Sense::drag());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0, HANDLE_BG);
        let y = rect.center().y;
        painter.hline(rect.x_range(), y, Stroke::new(2.0, HANDLE_LINE));
        let cx = rect.center().x;
        painter.add(egui::Shape::convex_polygon(
            vec![pos2(cx - 7.0, y - 2.0), pos2(cx, y - 8.0), pos2(cx + 7.0, y - 2.0)],
            TEXT_BRIGHT,
            Stroke::NONE,
        ));
        painter.add(egui::Shape::convex_polygon(
            vec![pos2(cx - 7.0, y + 2.0), pos2(cx + 7.0, y + 2.0), pos2(cx, y + 8.0)],
            TEXT_BRIGHT,
            Stroke::NONE,
        ));
        painter.text(pos2(cx + 48.0, y), Align2::CENTER_CENTER, "glisser", bold(11.0), TEXT);
        if response.hovered() || response.dragged() {
            ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
        }
        if response.dragged() {
            let max = (available - MIN_WAVE_HEIGHT - HANDLE_HEIGHT).max(MIN_VIDEO_HEIGHT);
            self.config.video_height =
                (self.config.video_height - response.drag_delta().y).clamp(MIN_VIDEO_HEIGHT, max);
        }
        if response.drag_stopped() {
            self.save_config();
            self.request_grab(self.view.playhead);
        }
    }

    fn video_panel(&mut self, ui: &mut Ui, height: f32) {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
        let pixels = ui.ctx().pixels_per_point();
        self.preview_box = ((rect.width() * pixels) as u32, (rect.height() * pixels) as u32);
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0, VIDEO_BG);
        match &self.video_texture {
            Some(texture) => {
                let [w, h] = texture.size();
                let target = waveform::fit_rect(rect, w as f32, h as f32);
                let uv = egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
                painter.image(texture.id(), target, uv, Color32::WHITE);
            }
            None => {
                painter.text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    "Aperçu vidéo",
                    FontId::proportional(15.0),
                    TEXT_DIM,
                );
            }
        }
        painter.rect_stroke(rect, 0, Stroke::new(1.0, BORDER), StrokeKind::Inside);
    }
}
