//! MateHub Desktop — полноценный десктоп-клиент (Phase 2.5+).
//!
//! Архитектура «Discord-модель»: интерфейс — существующий веб-фронт хаба,
//! загружаемый в webview (логин/SSO/чат/войс — тот же UI, что в браузере,
//! cookies персистентны); медиа-движок скриншаринга — нативный Rust
//! (scap → openh264 → matehub-rtc-client → SFU), доступный веб-фронту через
//! узкий мост `window.__MATEHUB_NATIVE__` (см. `bridge.js` и
//! `frontend/lib/native-bridge.ts`).
//!
//! Безопасность: remote-origin хаба получает через runtime capability
//! (`connect_hub`) ТОЛЬКО команды шаринга + core:event. Локальный пикер —
//! управление списком хабов. `start_share` дополнительно сверяет origin
//! `base_url` с активным хабом (runtime capabilities в Tauri 2.11 нельзя
//! отозвать — defense in depth на смену хаба).

mod audio;
mod capture;
mod dsp;
mod encoder;
mod hubs;
mod pipeline;
mod voice;

use std::sync::Arc;

use parking_lot::Mutex;
use tauri::ipc::CapabilityBuilder;
use tauri::{Manager, State, WebviewUrl, WebviewWindowBuilder};
use url::Url;

use crate::capture::TargetInfo;
use crate::hubs::HubStore;
use crate::pipeline::{PipelineHandle, ShareConfig};
use crate::voice::{VoiceConfig, VoiceHandle};

struct AppState {
    pipeline: Mutex<Option<PipelineHandle>>,
    voice: Mutex<Option<VoiceHandle>>,
    hubs: HubStore,
    active_hub: Arc<Mutex<Option<Url>>>,
}

/// Каноничный origin хаба: схема + хост + явный порт (если нестандартный).
fn origin_of(url: &Url) -> Result<String, String> {
    let host = url.host_str().ok_or("url has no host")?;
    let port = url.port().map(|p| format!(":{p}")).unwrap_or_default();
    Ok(format!("{}://{}{}", url.scheme(), host, port))
}

fn same_origin(a: &Url, b: &Url) -> bool {
    a.scheme() == b.scheme()
        && a.host_str() == b.host_str()
        && a.port_or_known_default() == b.port_or_known_default()
}

#[tauri::command]
fn capture_support() -> serde_json::Value {
    serde_json::json!({
        "supported": capture::is_supported(),
        "permission": capture::has_permission(),
    })
}

#[tauri::command]
fn request_capture_permission() -> bool {
    capture::request_permission()
}

#[tauri::command]
fn list_targets() -> Vec<TargetInfo> {
    capture::list_targets()
}

#[tauri::command]
fn list_hubs(state: State<'_, AppState>) -> Vec<String> {
    state.hubs.load()
}

/// Выдаёт origin'у хаба capability на команды шаринга и уводит webview
/// на его веб-интерфейс. Runtime capabilities аддитивны до перезапуска —
/// точечный набор пермишенов + origin-guard в `start_share` компенсируют
/// отсутствие revoke.
#[tauri::command]
fn connect_hub(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    url: String,
) -> Result<(), String> {
    let parsed: Url = url
        .trim()
        .parse()
        .map_err(|e| format!("invalid url: {e}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("hub url must be http(s)".into());
    }
    let origin = origin_of(&parsed)?;

    app.add_capability(
        CapabilityBuilder::new(format!("hub-{}", origin.replace(['/', ':'], "-")))
            .remote(origin.clone())
            .window("main")
            .permission("core:event:default")
            .permission("allow-capture-support")
            .permission("allow-request-capture-permission")
            .permission("allow-list-targets")
            .permission("allow-start-share")
            .permission("allow-stop-share")
            .permission("allow-leave-hub")
            .permission("allow-join-voice")
            .permission("allow-leave-voice")
            .permission("allow-set-mute")
            .permission("allow-set-deafen"),
    )
    .map_err(|e| format!("failed to grant hub capability: {e}"))?;

    if let Err(e) = state.hubs.remember(&origin) {
        tracing::warn!("failed to persist hub list: {e}");
    }
    *state.active_hub.lock() = Some(parsed.clone());

    tracing::info!(%origin, "navigating to hub");
    window.navigate(parsed).map_err(|e| e.to_string())
}

/// Возврат на пикер: глушим пайплайн и уходим на локальную страницу.
#[tauri::command]
fn leave_hub(window: tauri::WebviewWindow, state: State<'_, AppState>) -> Result<(), String> {
    if let Some(handle) = state.pipeline.lock().take() {
        handle.stop();
    }
    *state.active_hub.lock() = None;
    let url = WebviewUrl::App("index.html".into());
    let url: Url = url.to_string().parse().map_err(|e| format!("{e}"))?;
    window.navigate(url).map_err(|e| e.to_string())
}

#[tauri::command]
fn start_share(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    config: ShareConfig,
) -> Result<(), String> {
    if config.fps == 0 || config.fps > 120 {
        return Err("fps must be within 1..=120".into());
    }
    if config.bitrate_bps < 250_000 || config.bitrate_bps > 20_000_000 {
        return Err("bitrate must be within 250 kbps..=20 Mbps".into());
    }
    // Origin-guard: команду мог вызвать только разрешённый origin, но
    // capability старого хаба живёт до перезапуска — сверяем с активным.
    let active = state.active_hub.lock().clone();
    let Some(active) = active else {
        return Err("no active hub".into());
    };
    let base: Url = config
        .base_url
        .parse()
        .map_err(|e| format!("invalid base_url: {e}"))?;
    if !same_origin(&base, &active) {
        return Err("base_url origin does not match the active hub".into());
    }

    let mut slot = state.pipeline.lock();
    if slot.is_some() {
        return Err("share already running".into());
    }
    *slot = Some(pipeline::spawn(app, config));
    Ok(())
}

#[tauri::command]
fn stop_share(state: State<'_, AppState>) {
    if let Some(handle) = state.pipeline.lock().take() {
        handle.stop();
    }
}

/// Входит в голосовой канал нативно (микрофон + приём). На десктопе войс
/// принадлежит этому клиенту, webview-UI в WebRTC-войс НЕ входит.
#[tauri::command]
fn join_voice(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    config: VoiceConfig,
) -> Result<(), String> {
    let active = state.active_hub.lock().clone();
    let Some(active) = active else {
        return Err("no active hub".into());
    };
    let base: Url = config
        .base_url
        .parse()
        .map_err(|e| format!("invalid base_url: {e}"))?;
    if !same_origin(&base, &active) {
        return Err("base_url origin does not match the active hub".into());
    }
    let mut slot = state.voice.lock();
    if slot.is_some() {
        return Err("already in voice".into());
    }
    *slot = Some(voice::spawn(app, config));
    Ok(())
}

#[tauri::command]
fn leave_voice(state: State<'_, AppState>) {
    if let Some(handle) = state.voice.lock().take() {
        handle.stop();
    }
}

#[tauri::command]
fn set_mute(state: State<'_, AppState>, muted: bool) {
    if let Some(handle) = state.voice.lock().as_ref() {
        handle.set_mute(muted);
    }
}

#[tauri::command]
fn set_deafen(state: State<'_, AppState>, deafened: bool) {
    if let Some(handle) = state.voice.lock().as_ref() {
        handle.set_deafen(deafened);
    }
}

pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,str0m=warn".into()),
        )
        .init();

    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            capture_support,
            request_capture_permission,
            list_targets,
            list_hubs,
            connect_hub,
            leave_hub,
            start_share,
            stop_share,
            join_voice,
            leave_voice,
            set_mute,
            set_deafen
        ])
        .setup(|app| {
            let config_dir = app.path().app_config_dir()?;
            let active_hub: Arc<Mutex<Option<Url>>> = Arc::new(Mutex::new(None));
            app.manage(AppState {
                pipeline: Mutex::new(None),
                voice: Mutex::new(None),
                hubs: HubStore::new(config_dir),
                active_hub: active_hub.clone(),
            });

            let nav_hub = active_hub;
            WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                .title("MateHub")
                .inner_size(1280.0, 800.0)
                .min_inner_size(940.0, 600.0)
                .initialization_script(include_str!("bridge.js"))
                .on_navigation(move |url| {
                    // Локальные страницы: tauri://localhost (mac/linux),
                    // http(s)://tauri.localhost (windows).
                    if url.scheme() == "tauri"
                        || url.host_str() == Some("tauri.localhost")
                        || url.as_str() == "about:blank"
                    {
                        return true;
                    }
                    // Активный хаб — любые пути внутри его origin'а.
                    if let Some(hub) = nav_hub.lock().as_ref()
                        && same_origin(url, hub)
                    {
                        return true;
                    }
                    // Всё внешнее блокируем: webview — не браузер.
                    tracing::warn!(%url, "blocked navigation outside the active hub");
                    false
                })
                .build()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("failed to run tauri app");
}
