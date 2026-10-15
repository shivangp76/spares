//! `spares_frontend`: runs the web app's dev server, or the standalone image occlusion editor.
//!
//! # Image occlusion editor
//!
//! The editor is SVG-Edit (the `svgedit` submodule, a fork with the `ext-spares` extension). Its
//! initial style comes from `[image_occlusion.editor]` in `config.toml` (documented on
//! `ImageOcclusionEditorConfig` in `spares_core`). Press `?` in the editor, or choose
//! "Keyboard Shortcuts" in its main menu, to list its keyboard shortcuts.
//!
//! In the web app, it is embedded in an iframe with `?embedded=1`. The extension then hides its
//! own open/save tools and exposes `window.sparesBridge` to the frontend:
//! - In a note's editor (Notes → New note, or a note's detail), click "Insert image occlusion".
//!   Or open the "Image Occlusion" page to get a block to paste into a note written elsewhere.
//! - Choose, paste or drop an image, then draw clozes on the Clozes layer and markup on the
//!   Markup layer. Add a cloze settings string to clozes, as needed.
//! - Click "Save and insert". The server stores the image and clozes in its image occlusion
//!   directory and returns the block in the note's parser syntax.
//! - To change an existing image occlusion, open its note and click "Edit image occlusion N".
//!   Each save stores a new pair of files and updates the note, so the change can be undone.
//!   Stored files are never overwritten since card orders are written into them.
//!
//! The standalone editor (`spares_frontend image-occlusion`) is for those who only use the CLI:
//! - Run `spares_frontend image-occlusion [--image <PATH>]` (or `spares_frontend io`). The page
//!   opens with the image as its background, if given; otherwise, click "Change Background Image"
//!   and choose an image. Each run is its own instance, on the next free port.
//! - Add markup and clozes to the appropriate layer. Add cloze settings strings to clozes, as
//!   needed.
//! - Click "Save SVG", then use a snippet to insert the image occlusion into the note.
//!
//! Requirements of the standalone editor:
//! - Multiple instances can be run at once.
//! - It is a webpage, so that multiple instances can easily be managed. It is just a utility.
//! - It stays separate from the main UI, for people who only use the CLI, and so both can run
//!   at the same time.
//!
//! Potentially useful links:
//! - <https://github.com/SVG-Edit/svgedit>
//! - Method Draw (alternative to SVG-Edit): <https://github.com/methodofaction/Method-Draw/tree/master>
//!   - Addon using this Method Draw: <https://github.com/BlueGreenMagick/Image-Editor>
//! - SVG-Edit in Tauri: <https://github.com/brenoepics/svgedit-app>

use std::path::PathBuf;
use std::process::Command;
use std::process::exit;

use clap::CommandFactory;
use clap::Parser;
use clap::Subcommand;
use spares_core::api::image_occlusion::get_image_occlusion_editor_config;

#[derive(Parser)]
struct Args {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Open the standalone image occlusion editor
    #[command(visible_alias = "io")]
    ImageOcclusion {
        /// Image to open the editor with, as its background
        #[arg(long)]
        image: Option<PathBuf>,
    },
}

fn main() {
    // Answers shell completion requests (COMPLETE=zsh) and exits.
    clap_complete::CompleteEnv::with_factory(Args::command).complete();
    let args = Args::parse();
    let project_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let frontend_dir = project_root.join("frontend");
    let script = match args.command {
        Some(Commands::ImageOcclusion { .. }) => "start-image-occlusion",
        None => "start-main",
    };

    let mut command = Command::new("npm");
    command.args(["run", script]).current_dir(&frontend_dir);
    if let Some(Commands::ImageOcclusion { image }) = &args.command {
        // Served to the editor by the dev server. Each instance has its own.
        let config = get_image_occlusion_editor_config().unwrap_or_else(|e| {
            eprintln!("Failed to read the image occlusion editor config: {e}");
            exit(1);
        });
        command.env(
            "SPARES_IMAGE_OCCLUSION_EDITOR_CONFIG",
            serde_json::to_string(&config).expect("The config serializes"),
        );
        if let Some(image) = image {
            // The dev server runs elsewhere, so it is given an absolute path
            let image = image
                .canonicalize()
                .ok()
                .filter(|image| image.is_file())
                .unwrap_or_else(|| {
                    eprintln!("No image at {}", image.display());
                    exit(1);
                });
            command.env("SPARES_IMAGE_OCCLUSION_BACKGROUND", image);
        }
    }
    let status = command.status().expect("Failed to start npm process");

    if !status.success() {
        eprintln!("npm run {} failed with status: {}", script, status);
        exit(1);
    }
}
