//! Tiny shared helper around the `qrcode` crate — split out of `sync_cmds.rs` (Fase 4.2, the
//! Send/Pull QR) once `workspace_cmds.rs` needed the exact same rendering for the Fase 9.7 pairing
//! QR. Nothing app-specific lives here; both callers build their own JSON payload beforehand.

pub fn render_qr_svg(data: &str) -> Result<String, String> {
    let code = qrcode::QrCode::new(data.as_bytes()).map_err(|e| format!("failed to build QR code: {e}"))?;
    Ok(code
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(256, 256)
        .dark_color(qrcode::render::svg::Color("#000000"))
        .light_color(qrcode::render::svg::Color("#ffffff"))
        .build())
}
