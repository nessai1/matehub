//! Нативный голосовой клиент (Phase 2.5): микрофон + приём/микс/воспроизведение
//! чужих участников через тот же SFU. Снимает зависимость от WebRTC в webview
//! (на Linux он сломан). Веб-UI на десктопе в войс НЕ входит — источник
//! истины по медиа один: этот клиент (см. `docs/video/desktop-share.md`).
//!
//! Топология потоков:
//! ```text
//!  cpal in ─▶ input_rb ─▶ [mic] resample+20мс+Opus ─▶ AudioSender ─▶ SFU
//!  SFU ─▶ Publisher events ─▶ [tokio] RemoteAudio ─▶ remote_tx
//!  remote_tx ─▶ [mixer] Opus decode/каждый mid + 20мс микс ─▶ output_rb ─▶ cpal out
//! ```
//! cpal-колбэки realtime — общаются с рабочими потоками только через
//! lock-free кольца (`ringbuf`); тяжёлый кодек живёт в отдельных потоках.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use matehub_rtc_client::{
    EncodedAudioFrame, Publisher, PublisherConfig, PublisherEvent, VideoParams, ensure_session,
};
use opus::{Application, Channels, Decoder, Encoder};
use ringbuf::HeapRb;
use ringbuf::traits::{Consumer, Producer, Split};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tokio::sync::watch;

use crate::dsp::{FRAME_INTERLEAVED, FRAME_SAMPLES, RATE, dbov, mix_into, resample_linear};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceConfig {
    pub base_url: String,
    pub channel_id: String,
    pub hub_id: String,
    pub token: String,
}

/// Статус войса в webview (event `voice-status`).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum VoiceStatus {
    Connecting,
    Connected {
        participant_id: String,
    },
    ParticipantJoined {
        participant_id: String,
        user_id: String,
    },
    ParticipantLeft {
        participant_id: String,
        user_id: String,
    },
    Stopped {
        reason: String,
    },
    Error {
        message: String,
    },
}

fn emit(app: &AppHandle, status: &VoiceStatus) {
    if let Err(e) = app.emit("voice-status", status) {
        tracing::warn!("emit voice-status: {e}");
    }
}

pub struct VoiceHandle {
    stop_tx: watch::Sender<bool>,
    mute: Arc<AtomicBool>,
    deafen: Arc<AtomicBool>,
}

impl VoiceHandle {
    pub fn set_mute(&self, muted: bool) {
        self.mute.store(muted, Ordering::Relaxed);
    }
    pub fn set_deafen(&self, deafened: bool) {
        self.deafen.store(deafened, Ordering::Relaxed);
    }
    pub fn stop(&self) {
        let _ = self.stop_tx.send(true);
    }
}

pub fn spawn(app: AppHandle, cfg: VoiceConfig) -> VoiceHandle {
    let (stop_tx, stop_rx) = watch::channel(false);
    let mute = Arc::new(AtomicBool::new(false));
    let deafen = Arc::new(AtomicBool::new(false));
    let handle = VoiceHandle {
        stop_tx,
        mute: mute.clone(),
        deafen: deafen.clone(),
    };
    tauri::async_runtime::spawn(async move {
        if let Err(message) = run(app.clone(), cfg, stop_rx, mute, deafen).await {
            tracing::error!("voice failed: {message}");
            emit(&app, &VoiceStatus::Error { message });
        }
    });
    handle
}

async fn run(
    app: AppHandle,
    cfg: VoiceConfig,
    mut stop_rx: watch::Receiver<bool>,
    mute: Arc<AtomicBool>,
    deafen: Arc<AtomicBool>,
) -> Result<(), String> {
    emit(&app, &VoiceStatus::Connecting);

    let session_id = ensure_session(&cfg.base_url, &cfg.channel_id, &cfg.hub_id)
        .await
        .map_err(|e| e.to_string())?;
    let mut publisher = Publisher::connect(PublisherConfig {
        base_url: cfg.base_url.clone(),
        session_id,
        token: cfg.token.clone(),
        // device=None → основной participant-слот пользователя (не "screen").
        device: None,
        video: VideoParams::default(),
    })
    .await
    .map_err(|e| e.to_string())?;

    publisher
        .publish_microphone()
        .await
        .map_err(|e| e.to_string())?;
    emit(
        &app,
        &VoiceStatus::Connected {
            participant_id: publisher.participant_id().to_string(),
        },
    );

    let stopped = Arc::new(AtomicBool::new(false));
    let audio_tx = publisher.audio_sender();
    // Канал декодера: tokio-петля событий → mixer-поток.
    let (remote_tx, remote_rx) = std::sync::mpsc::channel::<(String, Arc<[u8]>)>();

    // Аудио-подсистема (cpal + mic-encode + mixer) целиком в std-потоках:
    // cpal::Stream — !Send, живёт на своём потоке.
    let audio_threads = start_audio(
        audio_tx,
        remote_rx,
        mute.clone(),
        deafen.clone(),
        stopped.clone(),
    )
    .map_err(|e| format!("audio subsystem: {e}"))?;

    let reason = loop {
        tokio::select! {
            event = publisher.next_event() => match event {
                Some(PublisherEvent::RemoteAudio { mid, data }) => {
                    // Дроп под давлением: mixer отстал — лучше потерять пакет.
                    let _ = remote_tx.send((mid, data));
                }
                Some(PublisherEvent::ParticipantJoined { participant_id, user_id }) => {
                    emit(&app, &VoiceStatus::ParticipantJoined {
                        participant_id: participant_id.to_string(), user_id,
                    });
                }
                Some(PublisherEvent::ParticipantLeft { participant_id, user_id }) => {
                    emit(&app, &VoiceStatus::ParticipantLeft {
                        participant_id: participant_id.to_string(), user_id,
                    });
                }
                Some(PublisherEvent::ServerError(message)) => {
                    emit(&app, &VoiceStatus::Error { message });
                }
                Some(PublisherEvent::Disconnected { reason }) => break reason,
                Some(_) => {}
                None => break "engine terminated".to_string(),
            },
            _ = stop_rx.changed() => {
                publisher.close();
                break "stopped by user".to_string();
            }
        }
    };

    stopped.store(true, Ordering::Relaxed);
    drop(remote_tx); // разбудит mixer-поток на выход
    for t in audio_threads {
        let _ = tokio::task::spawn_blocking(move || t.join()).await;
    }
    emit(&app, &VoiceStatus::Stopped { reason });
    Ok(())
}

/// Ёмкости колец (в сэмплах): ~200 мс запас — гасит джиттер, не раздувая лаг.
const RING_CAP: usize = FRAME_INTERLEAVED * 10;

fn start_audio(
    audio_tx: matehub_rtc_client::AudioSender,
    remote_rx: std::sync::mpsc::Receiver<(String, Arc<[u8]>)>,
    mute: Arc<AtomicBool>,
    deafen: Arc<AtomicBool>,
    stopped: Arc<AtomicBool>,
) -> Result<Vec<std::thread::JoinHandle<()>>, String> {
    // input: cpal in → mic. output: mixer → cpal out.
    let (in_prod, in_cons) = HeapRb::<f32>::new(RING_CAP).split();
    let (out_prod, out_cons) = HeapRb::<f32>::new(RING_CAP).split();

    // Поток cpal: строит и держит оба стрима (Stream не Send).
    let cpal_stopped = stopped.clone();
    let cpal_deafen = deafen.clone();
    let cpal_thread = std::thread::Builder::new()
        .name("matehub-voice-cpal".into())
        .spawn(move || {
            if let Err(e) = run_cpal(in_prod, out_cons, cpal_deafen, &cpal_stopped) {
                tracing::error!("cpal streams: {e}");
            }
        })
        .map_err(|e| e.to_string())?;

    // Поток микрофона: кольцо → resample/20мс/Opus → SFU.
    let mic_stopped = stopped.clone();
    let mic_thread = std::thread::Builder::new()
        .name("matehub-voice-mic".into())
        .spawn(move || mic_loop(in_cons, audio_tx, mute, &mic_stopped))
        .map_err(|e| e.to_string())?;

    // Поток микшера: Opus decode per-mid + 20мс тик → output.
    let mixer_thread = std::thread::Builder::new()
        .name("matehub-voice-mixer".into())
        .spawn(move || mixer_loop(remote_rx, out_prod, &stopped))
        .map_err(|e| e.to_string())?;

    Ok(vec![cpal_thread, mic_thread, mixer_thread])
}

/// Строит input+output cpal-стримы и держит их живыми до `stopped`.
fn run_cpal(
    mut in_prod: impl Producer<Item = f32> + Send + 'static,
    mut out_cons: impl Consumer<Item = f32> + Send + 'static,
    deafen: Arc<AtomicBool>,
    stopped: &AtomicBool,
) -> Result<(), String> {
    let host = cpal::default_host();

    let in_dev = host.default_input_device().ok_or("no input device")?;
    let in_cfg = in_dev.default_input_config().map_err(|e| e.to_string())?;
    let in_channels = in_cfg.channels() as usize;
    let in_rate = in_cfg.sample_rate().0;
    // cpal-колбэк входа: только перекладывание в кольцо (никакого кодека).
    let in_stream = in_dev
        .build_input_stream(
            &in_cfg.config(),
            move |data: &[f32], _| {
                in_prod.push_slice(data);
            },
            |e| tracing::warn!("input stream error: {e}"),
            None,
        )
        .map_err(|e| e.to_string())?;

    let out_dev = host.default_output_device().ok_or("no output device")?;
    let out_cfg = out_dev.default_output_config().map_err(|e| e.to_string())?;
    let out_stream = out_dev
        .build_output_stream(
            &out_cfg.config(),
            move |data: &mut [f32], _| {
                let n = if deafen.load(Ordering::Relaxed) {
                    0
                } else {
                    out_cons.pop_slice(data)
                };
                for s in data[n..].iter_mut() {
                    *s = 0.0; // underrun/deafen → тишина, колбэк не блокируем
                }
            },
            |e| tracing::warn!("output stream error: {e}"),
            None,
        )
        .map_err(|e| e.to_string())?;

    in_stream.play().map_err(|e| e.to_string())?;
    out_stream.play().map_err(|e| e.to_string())?;
    tracing::info!(in_rate, in_channels, "voice cpal streams started");

    // Держим стримы живыми; параметры входа кладём в глобал для mic_loop.
    IN_RATE.store(in_rate, Ordering::Relaxed);
    IN_CHANNELS.store(in_channels as u32, Ordering::Relaxed);
    while !stopped.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

// Параметры входного устройства, выставляются в run_cpal и читаются mic_loop.
static IN_RATE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(RATE);
static IN_CHANNELS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);

/// Микрофон: кольцо (device-формат) → stereo 48k → 20мс Opus → SFU.
fn mic_loop(
    mut in_cons: impl Consumer<Item = f32>,
    audio_tx: matehub_rtc_client::AudioSender,
    mute: Arc<AtomicBool>,
    stopped: &AtomicBool,
) {
    let mut enc = match Encoder::new(RATE, Channels::Stereo, Application::Voip) {
        Ok(e) => e,
        Err(e) => {
            tracing::error!("opus mic encoder: {e}");
            return;
        }
    };
    let mut scratch = vec![0f32; 4096];
    let mut acc: Vec<f32> = Vec::with_capacity(FRAME_INTERLEAVED * 4);
    let mut resample_pos = 0.0;
    let mut out_buf = vec![0u8; 4000];
    let mut rtp_ts: u64 = 0;

    while !stopped.load(Ordering::Relaxed) {
        let n = in_cons.pop_slice(&mut scratch);
        if n == 0 {
            std::thread::sleep(Duration::from_millis(5));
            continue;
        }
        let ch = IN_CHANNELS.load(Ordering::Relaxed).max(1) as usize;
        // device-формат (ch каналов, interleaved) → stereo interleaved.
        let mut stereo = Vec::with_capacity(n / ch * 2);
        for frame in scratch[..n].chunks_exact(ch) {
            let l = frame[0];
            let r = if ch > 1 { frame[1] } else { frame[0] };
            stereo.push(l);
            stereo.push(r);
        }
        let in_rate = IN_RATE.load(Ordering::Relaxed);
        let resampled = resample_linear(&stereo, in_rate, RATE, &mut resample_pos);
        acc.extend(resampled);

        while acc.len() >= FRAME_INTERLEAVED {
            let frame: Vec<f32> = acc.drain(..FRAME_INTERLEAVED).collect();
            // Mute: не шлём пакеты (SFU top-K нас не выберет — тишина).
            if mute.load(Ordering::Relaxed) {
                rtp_ts += FRAME_SAMPLES as u64;
                continue;
            }
            match enc.encode_float(&frame, &mut out_buf) {
                Ok(len) => {
                    let lvl = dbov(&frame);
                    audio_tx.send(EncodedAudioFrame {
                        data: out_buf[..len].into(),
                        rtp_time_48khz: rtp_ts,
                        captured_at: std::time::Instant::now(),
                        audio_level_dbov: lvl,
                        voice: lvl < 50,
                    });
                }
                Err(e) => tracing::warn!("opus mic encode: {e}"),
            }
            rtp_ts += FRAME_SAMPLES as u64;
        }
    }
}

/// Микшер: Opus-декод per-mid + 20мс тик → выходное кольцо.
fn mixer_loop(
    remote_rx: std::sync::mpsc::Receiver<(String, Arc<[u8]>)>,
    mut out_prod: impl Producer<Item = f32>,
    stopped: &AtomicBool,
) {
    // Небольшой playout-буфер на говорящего гасит джиттер (str0m уже
    // переупорядочил RTP — здесь только выравнивание к выходным часам).
    const MAX_QUEUED: usize = 5; // ~100мс
    let mut decoders: HashMap<String, (Decoder, std::collections::VecDeque<Vec<f32>>)> =
        HashMap::new();

    while !stopped.load(Ordering::Relaxed) {
        // Забираем все накопившиеся пакеты, декодируем в очереди по mid.
        loop {
            match remote_rx.try_recv() {
                Ok((mid, data)) => {
                    let entry = decoders.entry(mid).or_insert_with(|| {
                        (
                            Decoder::new(RATE, Channels::Stereo).expect("opus decoder init"),
                            std::collections::VecDeque::new(),
                        )
                    });
                    let mut pcm = vec![0f32; FRAME_INTERLEAVED];
                    match entry.0.decode_float(&data, &mut pcm, false) {
                        Ok(samples) => {
                            pcm.truncate(samples * 2); // samples на канал
                            if entry.1.len() >= MAX_QUEUED {
                                entry.1.pop_front(); // держим лаг ограниченным
                            }
                            entry.1.push_back(pcm);
                        }
                        Err(e) => tracing::warn!("opus decode: {e}"),
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => return,
            }
        }

        // 20мс тик: суммируем по одному кадру с каждого говорящего.
        let mut mixed = vec![0f32; FRAME_INTERLEAVED];
        let mut any = false;
        for (_, (_, queue)) in decoders.iter_mut() {
            if let Some(frame) = queue.pop_front() {
                mix_into(&mut mixed, &frame);
                any = true;
            }
        }
        if any {
            out_prod.push_slice(&mixed);
        }
        // Playout-часы: 20мс. (Простая версия v1; при дрейфе устройства
        // подстройка по заполненности кольца — следующий этап.)
        std::thread::sleep(Duration::from_millis(20));
    }
}
