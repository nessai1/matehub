//! Склейка capture → encode → publish.
//!
//! Два мира: std-поток (блокирующий scap + синхронный OpenH264) и tokio
//! (Publisher + события SFU). Мост — atomics для PLI/битрейта и
//! `FrameSender` (неблокирующий, с дропом под давлением).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

use matehub_rtc_client::{
    EncodedVideoFrame, Publisher, PublisherConfig, PublisherEvent, VideoParams, ensure_session,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tokio::sync::watch;

use crate::audio::AudioEncoder;
use crate::capture::{self, Capture, Captured};
use crate::encoder::{self, VideoEncoder};

/// camelCase: конфиг приходит из JS-моста (`frontend/lib/native-bridge.ts`).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareConfig {
    /// HTTP-база video-сервиса — origin активного хаба + `/api/video`
    /// (сверяется с активным хабом в `start_share`).
    pub base_url: String,
    pub channel_id: String,
    pub hub_id: String,
    pub token: String,
    /// Источник из пикера; `None` — основной дисплей.
    pub target: Option<TargetRef>,
    pub fps: u32,
    pub bitrate_bps: u32,
    /// Захватывать системный звук (macOS 13+/Windows; Linux — по возможности).
    #[serde(default)]
    pub system_audio: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetRef {
    pub kind: String,
    pub id: u64,
}

/// Статус в webview (event `share-status`).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ShareStatus {
    Connecting,
    Publishing {
        session_id: String,
    },
    Stopped {
        reason: String,
    },
    Error {
        message: String,
    },
    Stats {
        frames_sent: u64,
        frames_dropped: u64,
        target_bitrate_bps: u64,
    },
}

fn emit_status(app: &AppHandle, status: &ShareStatus) {
    if let Err(e) = app.emit("share-status", status) {
        tracing::warn!("emit share-status: {e}");
    }
}

/// Живёт, пока идёт шаринг; `stop()` идемпотентен.
pub struct PipelineHandle {
    stop_tx: watch::Sender<bool>,
}

impl PipelineHandle {
    pub fn stop(&self) {
        let _ = self.stop_tx.send(true);
    }
}

pub fn spawn(app: AppHandle, cfg: ShareConfig) -> PipelineHandle {
    let (stop_tx, stop_rx) = watch::channel(false);
    tauri::async_runtime::spawn(async move {
        if let Err(message) = run(app.clone(), cfg, stop_rx).await {
            tracing::error!("pipeline failed: {message}");
            emit_status(&app, &ShareStatus::Error { message });
        }
    });
    PipelineHandle { stop_tx }
}

async fn run(
    app: AppHandle,
    cfg: ShareConfig,
    mut stop_rx: watch::Receiver<bool>,
) -> Result<(), String> {
    emit_status(&app, &ShareStatus::Connecting);

    // Права на запись экрана: без них scap не отдаст кадры. preflight читает
    // реальное состояние TCC (по сигнатуре кода); request триггерит системный
    // запрос при первом обращении. macOS применяет грант только после
    // ПЕРЕЗАПУСКА приложения — сообщаем об этом явно, а не падаем.
    if !capture::has_permission() {
        capture::request_permission();
        return Err(
            "Screen recording permission is required. Grant it in System Settings → \
             Privacy & Security → Screen Recording, then fully quit and reopen MateHub."
                .into(),
        );
    }

    let session_id = ensure_session(&cfg.base_url, &cfg.channel_id, &cfg.hub_id)
        .await
        .map_err(|e| e.to_string())?;

    let mut publisher = Publisher::connect(PublisherConfig {
        base_url: cfg.base_url.clone(),
        session_id: session_id.clone(),
        token: cfg.token.clone(),
        // Отдельный participant-слот: не выбивает основную сессию юзера
        // (браузер/webview), которая сидит в том же звонке.
        device: Some("screen".into()),
        video: VideoParams {
            target_bitrate_bps: u64::from(cfg.bitrate_bps),
        },
    })
    .await
    .map_err(|e| e.to_string())?;

    publisher
        .publish_screen(cfg.system_audio)
        .await
        .map_err(|e| e.to_string())?;

    // Мосты в capture-поток.
    let force_idr = Arc::new(AtomicBool::new(false));
    let bwe_target = Arc::new(AtomicU64::new(u64::from(cfg.bitrate_bps)));
    let stopped = Arc::new(AtomicBool::new(false));
    let frames_sent = Arc::new(AtomicU64::new(0));
    let frames_dropped = Arc::new(AtomicU64::new(0));

    let frame_tx = publisher.frame_sender();
    let audio_tx = publisher.audio_sender();
    let capture_thread = {
        let (force_idr, bwe_target, stopped) =
            (force_idr.clone(), bwe_target.clone(), stopped.clone());
        let (frames_sent, frames_dropped) = (frames_sent.clone(), frames_dropped.clone());
        let cfg = cfg.clone();
        std::thread::Builder::new()
            .name("matehub-capture".into())
            .spawn(move || {
                capture_encode_loop(
                    &cfg,
                    &frame_tx,
                    &audio_tx,
                    &force_idr,
                    &bwe_target,
                    &stopped,
                    &frames_sent,
                    &frames_dropped,
                )
            })
            .map_err(|e| e.to_string())?
    };

    emit_status(
        &app,
        &ShareStatus::Publishing {
            session_id: session_id.clone(),
        },
    );

    let mut stats_tick = tokio::time::interval(std::time::Duration::from_secs(2));
    let reason = loop {
        tokio::select! {
            event = publisher.next_event() => match event {
                Some(PublisherEvent::KeyframeRequested) => {
                    force_idr.store(true, Ordering::Relaxed);
                }
                Some(PublisherEvent::TargetBitrate(bps)) => {
                    bwe_target.store(bps, Ordering::Relaxed);
                }
                Some(PublisherEvent::ServerError(message)) => {
                    tracing::warn!("SFU error: {message}");
                    emit_status(&app, &ShareStatus::Error { message });
                }
                Some(PublisherEvent::Disconnected { reason }) => break reason,
                Some(_) => {}
                None => break "engine terminated".to_string(),
            },
            _ = stop_rx.changed() => {
                publisher.close();
                break "stopped by user".to_string();
            }
            _ = stats_tick.tick() => {
                emit_status(&app, &ShareStatus::Stats {
                    frames_sent: frames_sent.load(Ordering::Relaxed),
                    frames_dropped: frames_dropped.load(Ordering::Relaxed),
                    target_bitrate_bps: bwe_target.load(Ordering::Relaxed),
                });
            }
        }
    };

    stopped.store(true, Ordering::Relaxed);
    // Capture-поток блокируется на get_next_frame ≤ 1/fps — дождёмся.
    let _ = tokio::task::spawn_blocking(move || capture_thread.join()).await;

    emit_status(&app, &ShareStatus::Stopped { reason });
    Ok(())
}

#[allow(clippy::too_many_arguments)] // capture-поток честно тащит все мосты
fn capture_encode_loop(
    cfg: &ShareConfig,
    frame_tx: &matehub_rtc_client::FrameSender,
    audio_tx: &matehub_rtc_client::AudioSender,
    force_idr: &AtomicBool,
    bwe_target: &AtomicU64,
    stopped: &AtomicBool,
    frames_sent: &AtomicU64,
    frames_dropped: &AtomicU64,
) {
    let target = cfg
        .target
        .as_ref()
        .and_then(|t| capture::find_target(&t.kind, t.id));

    let mut capture = match Capture::start(target, cfg.fps, cfg.system_audio) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("capture start failed: {e}");
            return;
        }
    };

    let mut encoder: Option<Box<dyn VideoEncoder>> = None;
    // Opus-стадия создаётся лениво на первом аудиокадре (Linux может не отдать
    // звук вовсе — тогда трек просто молчит).
    let mut audio_enc: Option<AudioEncoder> = None;
    let epoch = Instant::now();
    // Ретаргет — не чаще раза в ~2 секунды.
    let mut frames_since_retarget: u32 = 0;

    while !stopped.load(Ordering::Relaxed) {
        let captured = match capture.next_frame() {
            Ok(Some(c)) => c,
            Ok(None) => continue,
            Err(e) => {
                tracing::error!("capture failed: {e}");
                break;
            }
        };

        let raw = match captured {
            Captured::Video(v) => v,
            Captured::Audio(a) => {
                let enc = match &mut audio_enc {
                    Some(e) => e,
                    None => match AudioEncoder::new() {
                        Ok(e) => audio_enc.insert(e),
                        Err(e) => {
                            tracing::error!("audio encoder init failed: {e}");
                            continue;
                        }
                    },
                };
                enc.push(&a, |frame| {
                    audio_tx.send(frame);
                });
                continue;
            }
        };

        let enc = match &mut encoder {
            Some(e) => e,
            None => match encoder::select(raw.width, raw.height, cfg.fps, cfg.bitrate_bps) {
                Ok(e) => encoder.insert(e),
                Err(e) => {
                    tracing::error!("encoder init failed: {e}");
                    break;
                }
            },
        };

        if force_idr.swap(false, Ordering::Relaxed) {
            enc.force_keyframe();
        }
        frames_since_retarget += 1;
        if frames_since_retarget >= cfg.fps * 2 {
            frames_since_retarget = 0;
            if let Err(e) = enc.maybe_retarget(bwe_target.load(Ordering::Relaxed)) {
                tracing::warn!("bitrate retarget failed: {e}");
            }
        }

        let captured_at = Instant::now();
        match enc.encode(&raw) {
            Ok(Some(encoded)) => {
                let rtp_time_90khz = (epoch.elapsed().as_secs_f64() * 90_000.0).round() as u64;
                let delivered = frame_tx.send(EncodedVideoFrame {
                    data: encoded.annexb.into(),
                    rtp_time_90khz,
                    captured_at,
                    keyframe: encoded.keyframe,
                });
                if delivered {
                    frames_sent.fetch_add(1, Ordering::Relaxed);
                } else {
                    frames_dropped.fetch_add(1, Ordering::Relaxed);
                }
            }
            Ok(None) => {} // rate control пропустил кадр
            Err(e) => {
                tracing::warn!("encode failed: {e}");
            }
        }
    }

    capture.stop();
}
