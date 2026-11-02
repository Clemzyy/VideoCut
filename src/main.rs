// Pas de console derrière la fenêtre dans la version distribuée.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use videocut::app::{APP_TITLE, VideoCutApp};

fn main() -> eframe::Result {
    // `VideoCut.exe fichier.mp4` (ou « Ouvrir avec ») charge directement le fichier.
    let initial_file = std::env::args_os().nth(1).map(PathBuf::from);
    let viewport = egui::ViewportBuilder::default()
        .with_title(APP_TITLE)
        .with_inner_size([1100.0, 760.0])
        .with_min_inner_size([980.0, 660.0])
        .with_drag_and_drop(true)
        .with_icon(videocut::assets::window_icon());
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native("VideoCut", options, Box::new(move |cc| Ok(Box::new(VideoCutApp::new(cc, initial_file)))))
}
