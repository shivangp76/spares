use std::path::PathBuf;
use std::process::Command;
use std::process::exit;

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
