//! P78 fatia 1 — the hub's own web interface, served on the same port as the WebSocket protocol
//! (Jellyfin-style: open `http(s)://<hub>:<port>` in a browser and get the Warden UI). The page
//! lives in `web/` (its own React project, see `project/ARCHITECTURE.md`) and talks to the hub
//! over the same `Hello`/`Chat`/... protocol every other client uses, as one more paired device.
//!
//! Only what that needs, hand-rolled instead of pulling in an HTTP framework: read one request
//! head, answer `GET`/`HEAD` for a static file (or `index.html`, for a client-side route), close.
//! A request carrying `Upgrade: websocket` never gets here — `server.rs` hands it (with the bytes
//! already read, via `Rewind`) to tungstenite like before.

use std::borrow::Cow;
use std::collections::HashMap;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};

/// Where the page's files come from. The binaries use `EmbeddedWebUi`; tests use `StaticWebUi`.
pub trait WebAssets: Send + Sync {
    /// `path` is relative to the build root, no leading slash (`index.html`, `assets/app-1a2b.js`).
    fn get(&self, path: &str) -> Option<Cow<'static, [u8]>>;
}

/// `web/dist` compiled into the binary (read from disk at runtime in debug builds — `rust-embed`'s
/// default). `allow_missing`: a checkout that never ran `npm run build` in `web/` still compiles
/// and passes its tests; the hub then answers every page request with `NOT_BUILT_PAGE`.
#[derive(rust_embed::RustEmbed)]
#[folder = "../../web/dist"]
#[allow_missing = true]
pub struct EmbeddedWebUi;

impl WebAssets for EmbeddedWebUi {
    fn get(&self, path: &str) -> Option<Cow<'static, [u8]>> {
        <Self as rust_embed::RustEmbed>::get(path).map(|file| file.data)
    }
}

/// An in-memory set of files, keyed by path — for tests, or for embedding a UI some other way.
#[derive(Default)]
pub struct StaticWebUi(pub HashMap<String, Vec<u8>>);

impl WebAssets for StaticWebUi {
    fn get(&self, path: &str) -> Option<Cow<'static, [u8]>> {
        self.0.get(path).map(|bytes| Cow::Owned(bytes.clone()))
    }
}

/// Largest request head accepted — a browser's GET or a WebSocket upgrade is well under 2 KiB.
const MAX_HEAD_BYTES: usize = 8 * 1024;

/// How long a new connection gets to send its whole request head.
pub(crate) const HEAD_TIMEOUT: Duration = Duration::from_secs(10);

const NOT_BUILT_PAGE: &str = "<!doctype html><meta charset=utf-8><title>Warden</title>\
<p>This Warden hub has no web interface built in. Build it with <code>npm install &amp;&amp; npm run build</code> \
in <code>web/</code>, then rebuild the hub.</p>";

/// One request head, already read off the connection. `raw` is every byte read so far (the head
/// plus anything the client sent right after it), for handing back via `Rewind`.
pub(crate) struct RequestHead {
    pub raw: Vec<u8>,
    pub method: String,
    pub path: String,
    pub is_websocket_upgrade: bool,
}

/// Reads until the blank line ending the head. `Ok(None)` = the peer closed, sent garbage, or sent
/// more than `MAX_HEAD_BYTES` without finishing — nothing worth answering.
pub(crate) async fn read_request_head<S: AsyncRead + Unpin>(stream: &mut S) -> io::Result<Option<RequestHead>> {
    let mut raw = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    loop {
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            return Ok(None);
        }
        raw.extend_from_slice(&chunk[..n]);
        if let Some(parsed) = parse_head(&raw) {
            return Ok(parsed.map(|(method, path, is_websocket_upgrade)| RequestHead { raw, method, path, is_websocket_upgrade }));
        }
        if raw.len() > MAX_HEAD_BYTES {
            return Ok(None);
        }
    }
}

/// `None` = incomplete, read more. `Some(None)` = not HTTP. `Some(Some(..))` = method, path, and
/// whether it asks for a WebSocket upgrade.
fn parse_head(raw: &[u8]) -> Option<Option<(String, String, bool)>> {
    let mut headers = [httparse::EMPTY_HEADER; 64];
    let mut request = httparse::Request::new(&mut headers);
    match request.parse(raw) {
        Ok(httparse::Status::Partial) => None,
        Err(_) => Some(None),
        Ok(httparse::Status::Complete(_)) => {
            let (Some(method), Some(path)) = (request.method, request.path) else {
                return Some(None);
            };
            let upgrade = request
                .headers
                .iter()
                .any(|h| h.name.eq_ignore_ascii_case("upgrade") && String::from_utf8_lossy(h.value).to_ascii_lowercase().contains("websocket"));
            Some(Some((method.to_string(), path.to_string(), upgrade)))
        }
    }
}

/// Answers one plain HTTP request with a file from `assets`, then the connection is done.
pub(crate) async fn serve<S: AsyncWrite + Unpin>(stream: &mut S, head: &RequestHead, assets: &dyn WebAssets) -> io::Result<()> {
    let head_only = head.method == "HEAD";
    if head.method != "GET" && !head_only {
        return write_response(stream, "405 Method Not Allowed", &[("Allow", "GET, HEAD")], "text/plain; charset=utf-8", b"method not allowed", false).await;
    }
    let Some(index) = assets.get("index.html") else {
        return write_response(stream, "503 Service Unavailable", &[], "text/html; charset=utf-8", NOT_BUILT_PAGE.as_bytes(), head_only).await;
    };
    match resolve(&head.path) {
        Resolved::Index => write_response(stream, "200 OK", &[("Cache-Control", "no-cache")], "text/html; charset=utf-8", &index, head_only).await,
        Resolved::File(path) => match assets.get(&path) {
            Some(bytes) => {
                // Vite puts a content hash in every name under `assets/`, so those never change.
                let cache = if path.starts_with("assets/") { "public, max-age=31536000, immutable" } else { "no-cache" };
                write_response(stream, "200 OK", &[("Cache-Control", cache)], content_type(&path), &bytes, head_only).await
            }
            // No extension = a client-side route (`/skills`), which only `index.html` knows.
            None if !path.rsplit('/').next().unwrap_or_default().contains('.') => {
                write_response(stream, "200 OK", &[("Cache-Control", "no-cache")], "text/html; charset=utf-8", &index, head_only).await
            }
            None => not_found(stream, head_only).await,
        },
        Resolved::Refused => not_found(stream, head_only).await,
    }
}

/// A plain `http://` page request to a TLS-only hub: sent to the `https://` address when the hub
/// knows it, otherwise told it needs TLS (same refusal a plain `ws://` upgrade gets).
pub(crate) async fn redirect_to_https<S: AsyncWrite + Unpin>(stream: &mut S, head: &RequestHead, secure_url: Option<&str>) -> io::Result<()> {
    match secure_url.and_then(|url| url.strip_prefix("wss://")) {
        Some(authority) => {
            let location = format!("https://{authority}{}", head.path);
            write_response(stream, "308 Permanent Redirect", &[("Location", &location)], "text/plain; charset=utf-8", b"this hub only accepts encrypted connections (https://)", false).await
        }
        None => write_response(stream, "426 Upgrade Required", &[], "text/plain; charset=utf-8", b"this hub only accepts encrypted connections (https://)", false).await,
    }
}

async fn not_found<S: AsyncWrite + Unpin>(stream: &mut S, head_only: bool) -> io::Result<()> {
    write_response(stream, "404 Not Found", &[], "text/plain; charset=utf-8", b"not found", head_only).await
}

enum Resolved {
    Index,
    File(String),
    Refused,
}

/// Request target → asset path. Only plain relative segments get through: no `..`, `.`, empty
/// segments, or backslashes (the debug-build `EmbeddedWebUi` reads from disk, so this matters).
fn resolve(target: &str) -> Resolved {
    let path = target.split(['?', '#']).next().unwrap_or_default();
    let Some(path) = path.strip_prefix('/') else {
        return Resolved::Refused;
    };
    if path.is_empty() {
        return Resolved::Index;
    }
    let path = path.strip_suffix('/').unwrap_or(path);
    if path.contains('\\') || path.split('/').any(|segment| segment.is_empty() || segment == "." || segment == "..") {
        return Resolved::Refused;
    }
    Resolved::File(path.to_string())
}

fn content_type(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or_default().to_ascii_lowercase().as_str() {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json",
        "webmanifest" => "application/manifest+json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "wasm" => "application/wasm",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

async fn write_response<S: AsyncWrite + Unpin>(stream: &mut S, status: &str, extra: &[(&str, &str)], content_type: &str, body: &[u8], head_only: bool) -> io::Result<()> {
    let mut head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nX-Content-Type-Options: nosniff\r\nX-Frame-Options: DENY\r\nReferrer-Policy: no-referrer\r\nConnection: close\r\n",
        body.len()
    );
    for (name, value) in extra {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).await?;
    if !head_only {
        stream.write_all(body).await?;
    }
    stream.flush().await?;
    stream.shutdown().await
}

/// `inner`, with `prefix` (bytes already read off it) served again first — so tungstenite sees the
/// whole upgrade request even though `read_request_head` got to it first.
pub(crate) struct Rewind<S> {
    prefix: Vec<u8>,
    pos: usize,
    inner: S,
}

impl<S> Rewind<S> {
    pub fn new(prefix: Vec<u8>, inner: S) -> Self {
        Self { prefix, pos: 0, inner }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for Rewind<S> {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        if self.pos < self.prefix.len() {
            let n = (self.prefix.len() - self.pos).min(buf.remaining());
            let start = self.pos;
            buf.put_slice(&self.prefix[start..start + n]);
            self.pos += n;
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Rewind<S> {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolved(target: &str) -> Option<String> {
        match resolve(target) {
            Resolved::Index => Some("index.html".to_string()),
            Resolved::File(path) => Some(path),
            Resolved::Refused => None,
        }
    }

    #[test]
    fn resolve_maps_targets_to_asset_paths_and_refuses_traversal() {
        assert_eq!(resolved("/").as_deref(), Some("index.html"));
        assert_eq!(resolved("/?x=1").as_deref(), Some("index.html"));
        assert_eq!(resolved("/assets/app.js?v=2").as_deref(), Some("assets/app.js"));
        assert_eq!(resolved("/skills/").as_deref(), Some("skills"));
        assert_eq!(resolved("/../Cargo.toml"), None);
        assert_eq!(resolved("/assets/../../secret"), None);
        assert_eq!(resolved("/a//b"), None);
        assert_eq!(resolved("/a\\b"), None);
        assert_eq!(resolved("http://evil/x"), None);
    }

    #[test]
    fn parse_head_waits_for_the_blank_line_and_spots_upgrades() {
        assert!(parse_head(b"GET / HTTP/1.1\r\nHost: x\r\n").is_none());
        assert_eq!(parse_head(b"GET /a HTTP/1.1\r\nHost: x\r\n\r\n"), Some(Some(("GET".into(), "/a".into(), false))));
        let upgrade = b"GET / HTTP/1.1\r\nHost: x\r\nConnection: Upgrade\r\nUpgrade: WebSocket\r\n\r\n";
        assert_eq!(parse_head(upgrade), Some(Some(("GET".into(), "/".into(), true))));
        assert_eq!(parse_head(b"\x16\x03\x01garbage\r\n\r\n"), Some(None));
    }

    #[tokio::test]
    async fn rewind_replays_the_prefix_before_the_rest_of_the_stream() {
        let (mut client, server) = tokio::io::duplex(64);
        client.write_all(b" world").await.unwrap();
        drop(client);
        let mut rewound = Rewind::new(b"hello".to_vec(), server);
        let mut all = String::new();
        rewound.read_to_string(&mut all).await.unwrap();
        assert_eq!(all, "hello world");
    }
}
