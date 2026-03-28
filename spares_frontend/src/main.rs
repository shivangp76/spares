use clap::Parser;
use std::path::PathBuf;
use std::process::{Command, exit};

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

    let status = Command::new("npm")
        .args(["run", script])
        .current_dir(&frontend_dir)
        .status()
        .expect("Failed to start npm process");

    if !status.success() {
        eprintln!("npm run {} failed with status: {}", script, status);
        exit(1);
    }
}
