use crate::app::style::{ERROR, TEXT, TEXT_BRIGHT, bold};
use crate::app::{DialogKind, VideoCutApp};
use egui::{Align2, Color32, Context, Id, RichText, ViewportCommand};
use std::path::{Path, PathBuf};

impl VideoCutApp {
    pub(in crate::app) fn dialogs(&mut self, ctx: &Context) {
        let Some(dialog) = &self.dialog else { return };
        let mut close = false;
        let mut quit = false;
        let mut reveal = None;
        let response = egui::Modal::new(Id::unique("videocut-dialog")).show(ctx, |ui| {
            ui.set_max_width(520.0);
            let color = if matches!(dialog.kind, DialogKind::Error) { ERROR } else { TEXT_BRIGHT };
            ui.label(RichText::new(&dialog.title).font(bold(18.0)).color(color));
            ui.add_space(8.0);
            egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                ui.add(egui::Label::new(RichText::new(&dialog.body).color(TEXT)).selectable(true));
            });
            ui.add_space(12.0);
            ui.horizontal(|ui| match dialog.kind {
                DialogKind::ConfirmQuit => {
                    if ui.button("Quitter et annuler l'export").clicked() {
                        quit = true;
                    }
                    if ui.button("Continuer l'export").clicked() {
                        close = true;
                    }
                }
                _ => {
                    if ui.button("  OK  ").clicked() {
                        close = true;
                    }
                    if let Some(path) = &dialog.reveal {
                        if ui.button("Ouvrir le dossier").clicked() {
                            reveal = Some(path.clone());
                            close = true;
                        }
                    }
                }
            });
        });
        if response.should_close() && !matches!(dialog.kind, DialogKind::ConfirmQuit) {
            close = true;
        }
        if let Some(path) = reveal {
            reveal_in_folder(&path);
        }
        if quit {
            self.allow_close = true;
            self.dialog = None;
            ctx.send_viewport_cmd(ViewportCommand::Close);
        } else if close {
            self.dialog = None;
        }
    }
    pub(in crate::app) fn drop_overlay(&self, ctx: &Context) {
        if ctx.input(|i| i.raw.hovered_files.is_empty()) {
            return;
        }
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, Id::unique("drop-overlay")));
        let rect = ctx.content_rect();
        painter.rect_filled(rect, 0, Color32::from_black_alpha(170));
        painter.text(rect.center(), Align2::CENTER_CENTER, "Dépose le fichier pour l'ouvrir", bold(22.0), TEXT_BRIGHT);
    }
}

/// Ouvre l'Explorateur sur le fichier exporté.
fn reveal_in_folder(path: &Path) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let windir = std::env::var_os("WINDIR").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
        let _ = std::process::Command::new(windir.join("explorer.exe"))
            .raw_arg(format!("/select,\"{}\"", path.display()))
            .spawn();
    }
    #[cfg(not(windows))]
    if let Some(dir) = path.parent() {
        let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
    }
}
