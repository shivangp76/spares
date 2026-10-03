use std::path::PathBuf;
use std::process::Command;
use std::process::exit;

use clap::Parser;
use spares_core::api::image_occlusion::get_image_occlusion_editor_config;

#[derive(Parser)]
struct Args {
    #[arg(long)]
    image_occlusion: bool,
}

fn main() {
    let args = Args::parse();
    let project_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let frontend_dir = project_root.join("frontend");
    let script = if args.image_occlusion {
        "start-image-occlusion"
    } else {
        "start-main"
    };

    let mut command = Command::new("npm");
    command.args(["run", script]).current_dir(&frontend_dir);
    if args.image_occlusion {
        // Served to the editor by the dev server. Each instance has its own.
        let config = get_image_occlusion_editor_config().unwrap_or_else(|e| {
            eprintln!("Failed to read the image occlusion editor config: {e}");
            exit(1);
        });
        command.env(
            "SPARES_IMAGE_OCCLUSION_EDITOR_CONFIG",
            serde_json::to_string(&config).expect("The config serializes"),
        );
    }
    let status = command.status().expect("Failed to start npm process");

    if !status.success() {
        eprintln!("npm run {} failed with status: {}", script, status);
        exit(1);
    }
}
