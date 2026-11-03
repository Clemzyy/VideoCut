use super::{DialogKind, VideoCutApp};
use crate::media::VideoStream;
use crate::player::{self, GrabRequest, Playback, PlaybackRequest};
use crate::timefmt::{format_time, parse_time};
use crate::waveform::{Marker, MenuAction, WaveEvent};
use egui::{ColorImage, Context, TextureOptions};

impl VideoCutApp {
    fn preview_size_for(&self, video: &VideoStream) -> (u32, u32) {
        let max = (
            self.preview_box.0.clamp(320, player::PREVIEW_MAX.0),
            self.preview_box.1.clamp(180, player::PREVIEW_MAX.1),
        );
        player::preview_size((video.width, video.height), max)
    }

    pub(super) fn toggle_playback(&mut self, ctx: &Context) {
        if self.playback.is_some() {
            self.pause();
        } else {
            let (start, end, playhead) = (self.view.start, self.view.end, self.view.playhead);
            let from = if playhead < start || playhead >= end - 0.01 { start } else { playhead };
            self.play_from(from, ctx);
        }
    }

    fn play_from(&mut self, from: f64, ctx: &Context) {
        let (Some(media), Some(ffmpeg)) = (&self.media, &self.tools.ffmpeg) else { return };
        self.playback = None;
        let video_size = media.info.video.as_ref().map(|video| self.preview_size_for(video));
        let request = PlaybackRequest {
            ffmpeg,
            path: &media.path,
            info: &media.info,
            from,
            to: self.view.end,
            video_size,
            volume: self.volume.clone(),
            ctx: ctx.clone(),
        };
        match Playback::start(request) {
            Ok(mut playback) => {
                if let Some(warning) = playback.take_warning() {
                    self.status = warning;
                }
                self.view.set_playhead(from);
                self.playback = Some(playback);
            }
            Err(err) => self.show_dialog(DialogKind::Error, "Lecture impossible", err),
        }
    }

    pub(super) fn pause(&mut self) {
        self.playback = None;
    }

    pub(super) fn stop(&mut self) {
        self.playback = None;
        self.view.set_playhead(self.view.start);
        self.request_grab(self.view.start);
    }

    pub(super) fn poll_playback(&mut self, ctx: &Context) {
        let Some(playback) = &mut self.playback else { return };
        let error = playback.take_error();
        let position = playback.position();
        let frame = playback.take_frame();
        let finished = playback.is_finished() || position >= self.view.end - 1e-3;
        if let Some(image) = frame {
            self.set_video_image(ctx, image);
        }
        if let Some(err) = error {
            self.playback = None;
            self.show_dialog(DialogKind::Error, "Lecture interrompue", err);
            return;
        }
        if finished {
            self.playback = None;
            self.view.set_playhead(self.view.end);
        } else {
            self.view.follow_playhead(position);
        }
        ctx.request_repaint();
    }

    pub(super) fn request_grab(&mut self, time: f64) {
        let (Some(media), Some(ffmpeg)) = (&self.media, &self.tools.ffmpeg) else { return };
        let Some(video) = &media.info.video else { return };
        self.grabber.request(GrabRequest {
            generation: self.generation,
            ffmpeg: ffmpeg.clone(),
            path: media.path.clone(),
            stream_index: video.index,
            time,
            size: self.preview_size_for(video),
        });
    }

    pub(super) fn poll_grabber(&mut self, ctx: &Context) {
        while let Some(result) = self.grabber.try_recv() {
            if result.generation != self.generation || self.playback.is_some() {
                continue;
            }
            match result.image {
                Ok(image) => self.set_video_image(ctx, image),
                Err(err) => self.status = format!("Aperçu vidéo : {}", err.lines().last().unwrap_or_default()),
            }
        }
    }

    fn set_video_image(&mut self, ctx: &Context, image: ColorImage) {
        match &mut self.video_texture {
            Some(texture) => texture.set(image, TextureOptions::LINEAR),
            None => self.video_texture = Some(ctx.load_texture("video-preview", image, TextureOptions::LINEAR)),
        }
    }

    pub(super) fn on_wave_event(&mut self, event: WaveEvent, ctx: &Context) {
        match event {
            WaveEvent::DragStarted(_) => {
                if self.playback.take().is_some() {
                    self.resume_after_drag = true;
                }
            }
            WaveEvent::Moved(_, value) => {
                self.sync_time_fields();
                self.request_grab(value);
            }
            WaveEvent::DragFinished(marker) => {
                if std::mem::take(&mut self.resume_after_drag) {
                    // Après un réglage de la fin, réécouter les 2 dernières secondes.
                    let from = match marker {
                        Marker::End => (self.view.end - 2.0).max(self.view.start),
                        _ => self.view.playhead,
                    };
                    self.play_from(from, ctx);
                }
            }
            WaveEvent::Menu(action) => {
                self.playback = None;
                match action {
                    MenuAction::SetStart(time) => self.set_marker(Marker::Start, time),
                    MenuAction::SetEnd(time) => self.set_marker(Marker::End, time),
                    MenuAction::PlayFrom(time) => {
                        let from = time.clamp(self.view.start, (self.view.end - 0.05).max(self.view.start));
                        self.play_from(from, ctx);
                    }
                    MenuAction::SelectAll => {
                        self.view.set_end(self.view.duration());
                        self.set_marker(Marker::Start, 0.0);
                    }
                }
            }
        }
    }

    pub(super) fn set_marker(&mut self, marker: Marker, seconds: f64) {
        match marker {
            Marker::Start => {
                self.view.set_start(seconds);
                self.view.set_playhead(self.view.start);
            }
            Marker::End => {
                self.view.set_end(seconds);
                self.view.set_playhead(self.view.end);
            }
            Marker::Playhead => self.view.set_playhead(seconds),
        }
        self.sync_time_fields();
        self.request_grab(self.view.playhead);
    }

    pub(super) fn sync_time_fields(&mut self) {
        self.start_text = format_time(self.view.start);
        self.end_text = format_time(self.view.end);
    }

    pub(super) fn apply_time_field(&mut self, marker: Marker) {
        let text = if marker == Marker::Start { &self.start_text } else { &self.end_text };
        let parsed = parse_time(text).and_then(|value| {
            let ok = match marker {
                Marker::Start => value < self.view.end,
                _ => value > self.view.start && value <= self.view.duration() + 0.0005,
            };
            if ok { Ok(value) } else { Err("Le début doit précéder la fin, dans la durée du fichier.".into()) }
        });
        match parsed {
            Ok(value) => {
                self.playback = None;
                self.set_marker(marker, value);
            }
            Err(err) => {
                self.status = err;
                self.sync_time_fields();
            }
        }
    }
}
