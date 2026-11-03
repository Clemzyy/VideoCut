use egui::{Color32, Context, FontFamily, FontId, Stroke, vec2};
use std::path::PathBuf;
use std::sync::Arc;

pub(in crate::app) const BG: Color32 = Color32::from_rgb(0x15, 0x1a, 0x1f);
pub(in crate::app) const PANEL: Color32 = Color32::from_rgb(0x10, 0x14, 0x18);
pub(in crate::app) const BORDER: Color32 = Color32::from_rgb(0x2c, 0x35, 0x40);
pub(in crate::app) const TEXT: Color32 = Color32::from_rgb(0xd8, 0xe2, 0xea);
pub(in crate::app) const TEXT_BRIGHT: Color32 = Color32::from_rgb(0xea, 0xf3, 0xf8);
pub(in crate::app) const TEXT_DIM: Color32 = Color32::from_rgb(0x9f, 0xb1, 0xbf);
pub(in crate::app) const BUTTON_BG: Color32 = Color32::from_rgb(0x14, 0x21, 0x2a);
const BUTTON_HOVER: Color32 = Color32::from_rgb(0x1d, 0x34, 0x41);
pub(in crate::app) const HANDLE_BG: Color32 = Color32::from_rgb(0x26, 0x36, 0x42);
pub(in crate::app) const HANDLE_LINE: Color32 = Color32::from_rgb(0x49, 0x60, 0x6f);
pub(in crate::app) const VIDEO_BG: Color32 = Color32::from_rgb(0x05, 0x07, 0x08);
pub(in crate::app) const ERROR: Color32 = Color32::from_rgb(0xff, 0x7b, 0x72);

pub(in crate::app) fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("bold".into()))
}

pub(in crate::app) fn setup_fonts(ctx: &Context) {
    let mut fonts = egui::FontDefinitions::default();
    let windir = std::env::var_os("WINDIR").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    let mut load = |name: &str, file: &str| -> bool {
        match std::fs::read(windir.join("Fonts").join(file)) {
            Ok(bytes) => {
                fonts.font_data.insert(name.to_string(), Arc::new(egui::FontData::from_owned(bytes)));
                true
            }
            Err(_) => false,
        }
    };
    let has_regular = cfg!(windows) && load("segoe-ui", "segoeui.ttf");
    let has_bold = cfg!(windows) && load("segoe-ui-bold", "segoeuib.ttf");
    // egui n'embarque plus de police de symboles : ▶ ⏸ ⏹ ⚙ ⚠ viennent de Windows.
    let symbol_fonts: Vec<String> = [("segoe-ui-symbol", "seguisym.ttf"), ("segoe-ui-emoji", "seguiemj.ttf")]
        .into_iter()
        .filter(|(name, file)| cfg!(windows) && load(name, file))
        .map(|(name, _)| name.to_string())
        .collect();
    let monospace = fonts.families.entry(FontFamily::Monospace).or_default();
    monospace.extend(symbol_fonts.iter().cloned());
    let proportional = fonts.families.entry(FontFamily::Proportional).or_default();
    if has_regular {
        proportional.insert(0, "segoe-ui".to_string());
    }
    proportional.extend(symbol_fonts);
    let mut bold_family = proportional.clone();
    if has_bold {
        bold_family.insert(0, "segoe-ui-bold".to_string());
    }
    fonts.families.insert(FontFamily::Name("bold".into()), bold_family);
    ctx.set_fonts(fonts);
}

pub(in crate::app) fn setup_style(ctx: &Context) {
    ctx.set_theme(egui::Theme::Dark);
    ctx.style_mut_of(egui::Theme::Dark, |style| {
        let visuals = &mut style.visuals;
        visuals.panel_fill = BG;
        visuals.window_fill = PANEL;
        visuals.window_stroke = Stroke::new(1.0, BORDER);
        visuals.extreme_bg_color = PANEL;
        visuals.override_text_color = Some(TEXT);
        visuals.widgets.inactive.weak_bg_fill = BUTTON_BG;
        visuals.widgets.inactive.bg_fill = BUTTON_BG;
        visuals.widgets.hovered.weak_bg_fill = BUTTON_HOVER;
        visuals.widgets.hovered.bg_fill = BUTTON_HOVER;
        visuals.selection.bg_fill = Color32::from_rgb(0x1f, 0x6f, 0x99);
        style.spacing.button_padding = vec2(10.0, 6.0);
        style.spacing.item_spacing = vec2(8.0, 6.0);
        style.text_styles.insert(egui::TextStyle::Body, FontId::proportional(14.0));
        style.text_styles.insert(egui::TextStyle::Button, FontId::proportional(14.0));
    });
}
