// Anki's collection is a file and its API is a local process; neither exists in a browser.
#[cfg(not(target_family = "wasm"))]
pub mod anki;
pub mod spares;
