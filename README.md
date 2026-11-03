# VideoCut

**Découpeur et convertisseur audio/vidéo simple pour Windows**, écrit en Rust et propulsé par FFmpeg.

Choisis un fichier, place le début et la fin sur la forme d'onde, écoute ou regarde l'extrait, puis enregistre-le dans le format de ton choix.

## Fonctionnalités

- **Forme d'onde précise** : zoom à la molette jusqu'à la milliseconde, même sur un fichier de plusieurs heures. Le tracé reste fluide grâce à une pyramide de pics min/max affichée par la carte graphique.
- **Marqueurs Début / Fin** à glisser, ou à saisir au clavier (`01:23.500`).
- **Lecteur intégré** avec son et aperçu vidéo synchronisés sur l'horloge de la carte son. Le volume (0 à 200 %) s'applique en temps réel.
- **Aperçu vidéo redimensionnable** sous la forme d'onde, grâce à la barre « glisser ».
- **Deux modes d'export** :
  - *Précise* : réencodage, coupe à l'image près ;
  - *Rapide* : copie sans perte et quasi instantanée, mais le début se cale sur l'image-clé.
- **Conversion** vers MP4, MKV, MOV, WebM, AVI, MP3, M4A, WAV, FLAC, OGG, Opus et AAC.
- **Export sûr** : barre de progression, bouton Annuler, écriture atomique (jamais de fichier à moitié écrit) et contrôle du fichier produit. Il est impossible d'écraser le fichier source.
- **Glisser-déposer** d'un fichier dans la fenêtre, ou `VideoCut.exe mon_fichier.mp4`.
- **Mise à jour automatique de FFmpeg**, voir plus bas.

## Raccourcis

| Touche | Action |
|---|---|
| `Espace` | Lecture / pause |
| `I` / `O` | Placer le début / la fin à la tête de lecture |
| `Origine` | Revenir au début de la sélection |
| `Ctrl+O` | Ouvrir un fichier |
| `Ctrl+S` | Découper / convertir |
| Clic droit | Menu : placer le début ou la fin ici, lire depuis ici, zoomer sur la sélection, tout afficher |
| Molette | Zoom autour de la souris |
| `Maj` + molette, clic droit glissé | Se déplacer dans la forme d'onde |
| Double-clic | Voir tout le fichier |

## Installation

Télécharge `VideoCut.exe` depuis la page [Releases](https://github.com/Clemzyy/VideoCut/releases), puis lance-le. Aucune installation n'est nécessaire.

Au premier lancement, VideoCut télécharge FFmpeg automatiquement (environ 115 Mo, une seule fois).

## Mise à jour automatique des composants externes

VideoCut a un seul composant externe : **FFmpeg**. Il sert à l'analyse, à la lecture et à l'export.

- Il provient des versions officielles de [gyan.dev](https://www.gyan.dev/ffmpeg/builds/) publiées sur GitHub ([GyanD/codexffmpeg](https://github.com/GyanD/codexffmpeg/releases)).
- Une vérification a lieu au plus une fois par jour. Une nouvelle version est téléchargée en arrière-plan, puis contrôlée : taille, empreinte **SHA-256** annoncée par GitHub, et test de démarrage. Elle n'est activée qu'ensuite.
- L'ancienne version reste utilisable jusqu'au bout et n'est supprimée qu'au lancement suivant.
- FFmpeg est rangé dans `%LOCALAPPDATA%\VideoCut\ffmpeg\<version>\`.
- Le bouton ⚙ en bas à droite permet de désactiver la mise à jour, de vérifier tout de suite ou d'utiliser son propre `ffmpeg.exe`.

VideoCut signale aussi quand une nouvelle version de VideoCut est disponible sur ce dépôt.

Ordre de recherche de FFmpeg :

1. l'exécutable choisi à la main ;
2. la copie gérée par VideoCut ;
3. le dossier de `VideoCut.exe` ;
4. le `PATH`.

Le dossier courant n'est jamais utilisé.

## Compiler depuis les sources

Prérequis : [Rust](https://rustup.rs/) 1.85 ou plus récent, et les outils de compilation MSVC sous Windows.

```bash
cargo build --release
```

L'exécutable est produit dans `target/release/videocut.exe`.

Pour lancer les tests :

```bash
cargo test
```

Les tests d'intégration utilisent un vrai FFmpeg, trouvé dans `VIDEOCUT_FFMPEG_DIR` ou dans le `PATH`. Le test du téléchargement réel se lance à part :

```bash
cargo test --test updater_live -- --ignored
```

## Organisation du code

| Module | Rôle |
|---|---|
| `app/` | Fenêtre et interactions (egui) : chargement, lecture, export, mises à jour, panneaux d'interface |
| `waveform.rs` | Forme d'onde, zoom, marqueurs |
| `media.rs` | Analyse ffprobe et pyramide de pics |
| `player.rs` | Lecture audio/vidéo et aperçu d'image |
| `export.rs` | Découpe, conversion, progression, annulation |
| `updater.rs` | Mise à jour automatique de FFmpeg et de VideoCut |
| `tools.rs` | Localisation sûre de FFmpeg |
| `process.rs` | Lancement des processus (sans console, sans injection d'arguments) |

## Licence

VideoCut est distribué sous **licence MIT** : voir [LICENSE](LICENSE).

FFmpeg n'est **pas** inclus dans VideoCut. Il est téléchargé directement depuis gyan.dev, sous sa propre licence (GPL v3), dont le texte est enregistré à côté des exécutables.

## Auteur

**Clemzy** : [TikTok](https://www.tiktok.com/@__clemzy__) · [YouTube](https://www.youtube.com/@Clemzy) · [pc32.fr](https://www.pc32.fr/) · [Soutenir le projet (PayPal)](https://www.paypal.com/donate/?hosted_button_id=NKCR6KK739WGS)
