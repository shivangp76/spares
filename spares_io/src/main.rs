use std::path::PathBuf;
use std::process::{Command, exit};

fn main() {
    let project_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let frontend_dir = project_root.join("frontend");

    let status = Command::new("npm")
        .arg("run")
        .arg("main")
        .current_dir(&frontend_dir)
        .status()
        .expect("Failed to start npm process");

    if !status.success() {
        eprintln!("npm run main failed with status: {}", status);
        exit(1);
    }
}
