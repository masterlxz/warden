use std::net::Ipv4Addr;
use std::time::Duration;

const PROBE_TIMEOUT: Duration = Duration::from_millis(800);

/// Every host on the /24 of every local, non-loopback IPv4 interface — mirrors the sweep space
/// `sdk/dart/lib/src/internal/lan_sweep_client.dart` builds from `NetworkInterface.list()`.
/// Doesn't filter out virtual/container interfaces as carefully as the Dart client does yet
/// (v1 scope) — if that causes noisy sweeps in practice, tightening this is a cheap follow-up.
pub fn candidate_hosts() -> anyhow::Result<Vec<Ipv4Addr>> {
    let interfaces = if_addrs::get_if_addrs()?;
    let mut hosts = Vec::new();
    for iface in interfaces {
        if iface.is_loopback() {
            continue;
        }
        if let std::net::IpAddr::V4(addr) = iface.ip() {
            let octets = addr.octets();
            for last in 1..=254u8 {
                hosts.push(Ipv4Addr::new(octets[0], octets[1], octets[2], last));
            }
        }
    }
    hosts.sort();
    hosts.dedup();
    Ok(hosts)
}

fn client() -> anyhow::Result<reqwest::Client> {
    Ok(reqwest::Client::builder().timeout(PROBE_TIMEOUT).build()?)
}

/// Phase 1: `PUT http://{host}:{port}/session/{session_id}/content`, raw bytes as the body — no
/// JSON envelope, matching `lan_sweep_client.dart::putSessionContent`. `Ok(true)` means some
/// phone answered 200 (content delivered); `Ok(false)` means this host:port didn't answer (not
/// necessarily an error — the phone may just not be up yet, or not be this host at all).
pub async fn push_content(host: Ipv4Addr, port: u16, session_id: &str, blob: &[u8]) -> anyhow::Result<bool> {
    let url = format!("http://{host}:{port}/session/{session_id}/content");
    match client()?.put(&url).body(blob.to_vec()).send().await {
        Ok(resp) => Ok(resp.status().is_success()),
        Err(_) => Ok(false),
    }
}

/// Phase 2: `GET http://{host}:{port}/session/{session_id}`, expects `200 {"blob": "<base64>"}`,
/// matching `lan_sweep_client.dart::fetchSessionBlob`. `Ok(None)` means this host:port has no
/// result yet (or isn't the phone) — not necessarily an error.
pub async fn fetch_result(host: Ipv4Addr, port: u16, session_id: &str) -> anyhow::Result<Option<Vec<u8>>> {
    #[derive(serde::Deserialize)]
    struct Envelope {
        blob: String,
    }

    let url = format!("http://{host}:{port}/session/{session_id}");
    let resp = match client()?.get(&url).send().await {
        Ok(resp) => resp,
        Err(_) => return Ok(None),
    };
    if !resp.status().is_success() {
        return Ok(None);
    }
    let envelope: Envelope = resp.json().await?;
    let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &envelope.blob)?;
    Ok(Some(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::{get, put};
    use axum::Router;

    #[test]
    fn candidate_hosts_runs_without_error_on_this_machine() {
        // Can't assert specific addresses (depends on the machine running the test), but this
        // proves if-addrs enumeration + /24 expansion doesn't panic and excludes loopback.
        let hosts = candidate_hosts().unwrap();
        assert!(!hosts.contains(&Ipv4Addr::new(127, 0, 0, 1)));
    }

    async fn spawn_test_server(router: Router) -> u16 {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        port
    }

    #[tokio::test]
    async fn push_content_delivers_the_raw_body() {
        use std::sync::{Arc, Mutex};

        let received: Arc<Mutex<Option<Vec<u8>>>> = Arc::new(Mutex::new(None));
        let received_clone = received.clone();
        let router = Router::new().route(
            "/session/{id}/content",
            put(move |body: axum::body::Bytes| {
                let received = received_clone.clone();
                async move {
                    *received.lock().unwrap() = Some(body.to_vec());
                    axum::http::StatusCode::OK
                }
            }),
        );
        let port = spawn_test_server(router).await;

        let ok = push_content(Ipv4Addr::LOCALHOST, port, "abc123", b"hello")
            .await
            .unwrap();
        assert!(ok);
        assert_eq!(received.lock().unwrap().as_deref(), Some(&b"hello"[..]));
    }

    #[tokio::test]
    async fn fetch_result_decodes_the_base64_envelope() {
        let router = Router::new().route(
            "/session/{id}",
            get(|| async { r#"{"blob":"aGVsbG8="}"# }),
        );
        let port = spawn_test_server(router).await;

        let result = fetch_result(Ipv4Addr::LOCALHOST, port, "abc123")
            .await
            .unwrap();
        assert_eq!(result, Some(b"hello".to_vec()));
    }

    #[tokio::test]
    async fn fetch_result_returns_none_when_nothing_is_listening() {
        let result = fetch_result(Ipv4Addr::LOCALHOST, 65500, "abc123")
            .await
            .unwrap();
        assert_eq!(result, None);
    }
}
