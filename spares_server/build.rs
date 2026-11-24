//! Rebuilds when an accounts migration is added: `sqlx::migrate!` embeds them at compile time, and
//! cargo doesn't know it reads them. The collection's are `spares_core`'s.
fn main() {
    println!("cargo:rerun-if-changed=accounts_migrations");
}
