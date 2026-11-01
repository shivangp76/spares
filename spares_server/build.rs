//! Rebuilds when a migration is added: `sqlx::migrate!` embeds them at compile time, and cargo
//! doesn't know it reads them.
fn main() {
    for dir in ["../spares_core/migrations", "accounts_migrations"] {
        println!("cargo:rerun-if-changed={dir}");
    }
}
