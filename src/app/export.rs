use super::{Dialog, DialogKind, VideoCutApp, file_name};
use crate::export::{self, ExportHandle, ExportJob, ExportMode, ExportOutcome, FORMATS};
use egui::Context;

impl VideoCutApp {
    pub(super) fn export_clip(&mut self, ctx: &Context) {
        if self.export.is_some() {
            return;
        }
        let (Some(media), Some(ffmpeg)) = (&self.media, self.tools.ffmpeg.clone()) else { return };
        self.playback = None;
        let source = media.path.clone();
        let extension = export::extension_of(&source);
        let stem = source.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "extrait".into());
        let mut dialog = rfd::FileDialog::new()
            .set_title("Enregistrer l'extrait")
            .set_file_name(format!("{stem}_extrait.{}", if extension.is_empty() { "mp4" } else { &extension }));
        if let Some(dir) = source.parent() {
            dialog = dialog.set_directory(dir);
        }
        if !extension.is_empty() {
            dialog = dialog.add_filter(format!("Même format (.{extension})"), &[extension.as_str()]);
        }
        for (label, ext) in FORMATS {
            dialog = dialog.add_filter(*label, &[*ext]);
        }
        let Some(mut output) = dialog.add_filter("Tous les fichiers", &["*"]).save_file() else { return };
        if output.extension().is_none() {
            output.set_extension(if extension.is_empty() { "mp4" } else { &extension });
        }
        let job = ExportJob {
            ffmpeg,
            ffprobe: self.tools.ffprobe.clone(),
            source,
            output,
            start: self.view.start,
            end: self.view.end,
            mode: if self.config.precise_export { ExportMode::Precise } else { ExportMode::Fast },
            info: media.info.clone(),
        };
        if let Err(err) = export::validate(&job) {
            self.show_dialog(DialogKind::Error, "Export impossible", err);
            return;
        }
        self.status = format!("Découpage en cours vers {}…", file_name(&job.output));
        self.export = Some(export::spawn(job, ctx.clone()));
    }

    pub(super) fn poll_export(&mut self) {
        let Some(outcome) = self.export.as_ref().and_then(ExportHandle::try_outcome) else { return };
        self.export = None;
        match outcome {
            ExportOutcome::Done { output, notes } => {
                self.status = format!("Extrait créé : {}", output.display());
                let mut body = format!("Extrait créé :\n{}", output.display());
                for note in notes {
                    body.push_str("\n\n⚠ ");
                    body.push_str(&note);
                }
                self.dialog =
                    Some(Dialog { title: "Terminé".into(), body, kind: DialogKind::Info, reveal: Some(output) });
            }
            ExportOutcome::Failed(err) => {
                self.status = "Erreur pendant le découpage.".into();
                self.show_dialog(DialogKind::Error, "Erreur FFmpeg", err);
            }
            ExportOutcome::Cancelled => self.status = "Export annulé : aucun fichier n'a été laissé.".into(),
        }
    }
}
