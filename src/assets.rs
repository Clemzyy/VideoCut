//! Icônes embarquées dans l'exécutable : icône du logiciel en 256 px sur
//! fond transparent et logos sociaux pour fond sombre (mêmes que StreamDL).

use egui::{ColorImage, Context, IconData, TextureFilter, TextureHandle, TextureOptions};

const SELECT_PNG: &[u8] = include_bytes!("../assets/select_256.png");
const APP_PNG: &[u8] = include_bytes!("../assets/Icone_256.png");
const TIKTOK_PNG: &[u8] = include_bytes!("../assets/social_tiktok.png");
const YOUTUBE_PNG: &[u8] = include_bytes!("../assets/social_youtube.png");
const SITE_PNG: &[u8] = include_bytes!("../assets/social_pc32.png");
const PAYPAL_PNG: &[u8] = include_bytes!("../assets/social_paypal.png");
const GITHUB_PNG: &[u8] = include_bytes!("../assets/social_github.png");

pub(crate) struct Link {
    pub label: &'static str,
    pub fallback: &'static str,
    pub url: &'static str,
}

/// Liens de la barre du haut, de gauche à droite (GitHub tout à droite).
pub(crate) const LINKS: [Link; 5] = [
    Link { label: "TikTok Clemzy", fallback: "TikTok", url: "https://www.tiktok.com/@__clemzy__" },
    Link { label: "YouTube Clemzy", fallback: "YouTube", url: "https://www.youtube.com/@Clemzy" },
    Link { label: "Site internet pc32.fr", fallback: "pc32", url: "https://www.pc32.fr/" },
    Link {
        label: "Donation PayPal",
        fallback: "PayPal",
        url: "https://www.paypal.com/donate/?hosted_button_id=NKCR6KK739WGS",
    },
    Link {
        label: "Code source de VideoCut sur GitHub",
        fallback: "GitHub",
        url: "https://github.com/Clemzyy/VideoCut",
    },
];

pub(crate) struct Icons {
    pub select: Option<TextureHandle>,
    pub export: Option<TextureHandle>,
    /// Dans l'ordre de [`LINKS`].
    pub links: [Option<TextureHandle>; 5],
}

impl Icons {
    pub(crate) fn load(ctx: &Context) -> Self {
        // Mipmaps : l'icône 256 px reste nette une fois réduite à l'écran.
        let options = TextureOptions { mipmap_mode: Some(TextureFilter::Linear), ..TextureOptions::LINEAR };
        let texture = |name: &str, bytes: &[u8]| decode_png(bytes).map(|image| ctx.load_texture(name, image, options));
        Self {
            select: texture("select", SELECT_PNG),
            export: texture("export", APP_PNG),
            links: [
                texture("tiktok", TIKTOK_PNG),
                texture("youtube", YOUTUBE_PNG),
                texture("site", SITE_PNG),
                texture("paypal", PAYPAL_PNG),
                texture("github", GITHUB_PNG),
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
    image::load_from_memory_with_format(APP_PNG, image::ImageFormat::Png)
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
        for bytes in [SELECT_PNG, APP_PNG, TIKTOK_PNG, YOUTUBE_PNG, SITE_PNG, PAYPAL_PNG, GITHUB_PNG] {
            let image = decode_png(bytes).expect("PNG valide");
            assert!(image.size[0] >= 32 && image.size[1] >= 32);
        }
        let icon = window_icon();
        assert_eq!((icon.width, icon.height), (256, 256));
        assert_eq!(icon.rgba.len(), 256 * 256 * 4);
    }

    #[test]
    fn app_icons_have_a_transparent_background() {
        for bytes in [APP_PNG, SELECT_PNG] {
            let image = decode_png(bytes).unwrap();
            assert_eq!(image.size, [256, 256]);
            assert_eq!(image.pixels[0].a(), 0, "le coin doit être transparent");
        }
    }

    #[test]
    fn links_are_https() {
        for link in &LINKS {
            assert!(link.url.starts_with("https://"), "{}", link.url);
        }
    }
}
