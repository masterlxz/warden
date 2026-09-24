//! Tells Cargo when to recompile so `web_ui::EmbeddedWebUi` (P78) picks up a fresh `npm run build`
//! in `web/`: `rust-embed` reads `web/dist` at compile time, and Cargo alone doesn't notice files
//! appearing there (every build gets new content-hashed names). While `web/dist` doesn't exist yet,
//! watch `web/` instead, so creating it triggers the rebuild.

fn main() {
    let dist = std::path::Path::new("../../web/dist");
    if dist.is_dir() {
        println!("cargo:rerun-if-changed=../../web/dist");
    } else {
        println!("cargo:rerun-if-changed=../../web");
    }
}
