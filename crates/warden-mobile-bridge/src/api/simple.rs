/// Toolchain smoke test (Fase 4.4, Estágio A) — proves the Android-compiled `.so` links and runs
/// inside the Flutter app before any real sync logic is wired in. Superseded by the real
/// `SyncEngine` bridge functions in Estágio B; kept as a cheap sanity check even after that lands.
#[flutter_rust_bridge::frb(sync)]
pub fn ping() -> String {
    "pong from warden-mobile-bridge".to_string()
}

#[flutter_rust_bridge::frb(init)]
pub fn init_app() {
    // Default utilities - feel free to customize
    flutter_rust_bridge::setup_default_user_utils();
}
