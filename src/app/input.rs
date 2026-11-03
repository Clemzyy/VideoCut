use super::{DialogKind, VideoCutApp};
use crate::waveform::Marker;
use egui::{Context, Key, Modifiers, ViewportCommand};

impl VideoCutApp {
    pub(super) fn handle_input(&mut self, ctx: &Context) {
        if let Some(path) = ctx.input(|i| i.raw.dropped_files.first().map(|file| file.path().to_path_buf())) {
            self.open_file(path, ctx);
        }
        if ctx.input(|i| i.viewport().close_requested()) && self.export.is_some() && !self.allow_close {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            self.show_dialog(
                DialogKind::ConfirmQuit,
                "Export en cours",
                "Un découpage est en cours. Quitter maintenant l'annulera.",
            );
        }
        if ctx.egui_wants_keyboard_input() || self.dialog.is_some() {
            return;
        }
        let pressed = |key: Key, modifiers: Modifiers| ctx.input_mut(|i| i.consume_key(modifiers, key));
        if pressed(Key::O, Modifiers::COMMAND) {
            self.choose_file(ctx);
        }
        if self.media.is_none() {
            return;
        }
        if pressed(Key::Space, Modifiers::NONE) {
            self.toggle_playback(ctx);
        }
        if pressed(Key::I, Modifiers::NONE) {
            self.set_marker(Marker::Start, self.view.playhead);
        }
        if pressed(Key::O, Modifiers::NONE) {
            self.set_marker(Marker::End, self.view.playhead);
        }
        if pressed(Key::Home, Modifiers::NONE) {
            self.stop();
        }
        if pressed(Key::S, Modifiers::COMMAND) || pressed(Key::E, Modifiers::COMMAND) {
            self.export_clip(ctx);
        }
    }
}
