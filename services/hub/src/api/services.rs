// Cross-service version aggregator. The hub service knows which peers
// exist (from compile-time list) and where to find them (env vars set per
// deployment); it pings each peer's /version with a tight timeout so a
// down peer doesn't slow the popup. Frontend renders the response as the
// "Services" panel in the hub-switcher dropdown.

use std::time::Duration;

use axum::{Json, Router, routing::get};
use futures_util::future::join_all;
use serde::Serialize;

#[derive(Serialize, Clone)]
pub struct ServiceInfo {
    pub name: String,
    pub version: Option<String>,
    pub status: ServiceStatus,
}

#[derive(Serialize, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ServiceStatus {
    Up,
    Down,
}

// (display_name, env_var). Env var holds the peer base URL (e.g.
// http://chat:3001). For the hub service we report ourselves directly.
//
// As feature services come online (e.g. kanban, scheduler) they get
// added here — the frontend renders whatever the API returns, so no
// frontend change required.
const PEERS: &[(&str, &str)] = &[
    ("chat", "PEER_CHAT_URL"),
    ("video", "PEER_VIDEO_URL"),
    ("general", "PEER_GENERAL_URL"),
    ("transcoder", "PEER_TRANSCODER_URL"),
];

pub fn routes() -> Router {
    Router::new().route("/services/versions", get(list_services))
}

async fn list_services() -> Json<Vec<ServiceInfo>> {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_millis(800))
        .build()
    {
        Ok(c) => c,
        Err(_) => {
            return Json(vec![hub_self()]);
        }
    };

    let peer_futs = PEERS.iter().map(|(name, env_key)| {
        let client = client.clone();
        async move {
            let Ok(base) = std::env::var(env_key) else {
                return ServiceInfo {
                    name: (*name).to_string(),
                    version: None,
                    status: ServiceStatus::Down,
                };
            };
            fetch_peer(&client, name, &base).await
        }
    });

    let mut out = vec![hub_self()];
    out.extend(join_all(peer_futs).await);
    Json(out)
}

fn hub_self() -> ServiceInfo {
    ServiceInfo {
        name: "hub".to_string(),
        version: Some(
            option_env!("MATEHUB_VERSION")
                .unwrap_or(env!("CARGO_PKG_VERSION"))
                .to_string(),
        ),
        status: ServiceStatus::Up,
    }
}

async fn fetch_peer(client: &reqwest::Client, name: &str, base: &str) -> ServiceInfo {
    let url = format!("{}/version", base.trim_end_matches('/'));
    match client.get(&url).send().await {
        Ok(resp) if resp.status().is_success() => {
            let version = resp
                .json::<serde_json::Value>()
                .await
                .ok()
                .and_then(|v| v.get("version").and_then(|s| s.as_str()).map(String::from));
            ServiceInfo {
                name: name.to_string(),
                version,
                status: ServiceStatus::Up,
            }
        }
        _ => ServiceInfo {
            name: name.to_string(),
            version: None,
            status: ServiceStatus::Down,
        },
    }
}
