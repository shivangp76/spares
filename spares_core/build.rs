//! Rebuilds when a migration is added: `sqlx::migrate!` embeds them at compile time, and cargo
//! doesn't know it reads them.
fn main() {
    println!("cargo:rerun-if-changed=migrations");
}
