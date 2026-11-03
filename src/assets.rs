//! Icônes embarquées dans l'exécutable : les fichiers fournis par Clemzy,
//! utilisés tels quels.

use egui::{ColorImage, Context, IconData, TextureHandle, TextureOptions};

const SELECT_PNG: &[u8] = include_bytes!("../assets/select_app.png");
const EXPORT_PNG: &[u8] = include_bytes!("../assets/Icone_app.png");
const WINDOW_PNG: &[u8] = include_bytes!("../assets/Icone_256.png");
const TIKTOK_PNG: &[u8] = include_bytes!("../assets/social_tiktok_32.png");
const YOUTUBE_PNG: &[u8] = include_bytes!("../assets/social_youtube_32.png");
const SITE_PNG: &[u8] = include_bytes!("../assets/social_pc32_32.png");
const PAYPAL_PNG: &[u8] = include_bytes!("../assets/social_paypal_32.png");

pub(crate) struct Link {
    pub label: &'static str,
    pub fallback: &'static str,
    pub url: &'static str,
}

pub(crate) const LINKS: [Link; 4] = [
    Link { label: "TikTok Clemzy", fallback: "TikTok", url: "https://www.tiktok.com/@__clemzy__" },
    Link { label: "YouTube Clemzy", fallback: "YouTube", url: "https://www.youtube.com/@Clemzy" },
    Link { label: "Site internet pc32.fr", fallback: "pc32", url: "https://www.pc32.fr/" },
    Link {
        label: "Donation PayPal",
        fallback: "PayPal",
        url: "https://www.paypal.com/donate/?hosted_button_id=NKCR6KK739WGS",
    },
];

pub(crate) struct Icons {
    pub select: Option<TextureHandle>,
    pub export: Option<TextureHandle>,
    /// Dans l'ordre de [`LINKS`].
    pub links: [Option<TextureHandle>; 4],
}

impl Icons {
    pub(crate) fn load(ctx: &Context) -> Self {
        let texture = |name: &str, bytes: &[u8]| {
            decode_png(bytes).map(|image| ctx.load_texture(name, image, TextureOptions::LINEAR))
        };
        Self {
            select: texture("select", SELECT_PNG),
            export: texture("export", EXPORT_PNG),
            links: [
                texture("tiktok", TIKTOK_PNG),
                texture("youtube", YOUTUBE_PNG),
                texture("site", SITE_PNG),
                texture("paypal", PAYPAL_PNG),
            ],
        }
    }
}

pub(crate) fn decode_png(bytes: &[u8]) -> Option<ColorImage> {
    let image = image::load_from_memory_with_format(bytes, image::ImageFormat::Png).ok()?.to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    Some(ColorImage::from_rgba_unmultiplied(size, image.as_raw()))
}

/// Icône de la fenêtre et de la barre des tâches.
pub fn window_icon() -> IconData {
    image::load_from_memory_with_format(WINDOW_PNG, image::ImageFormat::Png)
        .map(|image| {
            let image = image.to_rgba8();
            IconData { width: image.width(), height: image.height(), rgba: image.into_raw() }
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_embedded_icon_decodes() {
        for bytes in [SELECT_PNG, EXPORT_PNG, WINDOW_PNG, TIKTOK_PNG, YOUTUBE_PNG, SITE_PNG, PAYPAL_PNG] {
            let image = decode_png(bytes).expect("PNG valide");
            assert!(image.size[0] >= 32 && image.size[1] >= 32);
        }
        let icon = window_icon();
        assert_eq!((icon.width, icon.height), (256, 256));
        assert_eq!(icon.rgba.len(), 256 * 256 * 4);
    }

    #[test]
    fn links_are_https() {
        for link in &LINKS {
            assert!(link.url.starts_with("https://"), "{}", link.url);
        }
    }
}
