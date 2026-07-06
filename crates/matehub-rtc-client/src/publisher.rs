//! Публичный асинхронный фасад: WS-сигналинг + engine-поток.

use std::collections::VecDeque;
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use uuid::Uuid;

use crate::core::EngineCore;
use crate::engine::{self, CtrlCmd, DataCmd};
use crate::error::RtcClientError;
use crate::signaling::{ClientMessage, ServerMessage, ws_url};
use crate::types::{EncodedAudioFrame, EncodedVideoFrame, PublisherEvent, VideoParams};

#[derive(Debug, Clone)]
pub struct PublisherConfig {
    /// HTTP-база video-сервиса: `http://localhost:4000` или
    /// `https://hub.example.com/api/video`.
    pub base_url: String,
    /// UUID сессии (из `POST /v1/sessions`, см. [`ensure_session`]).
    pub session_id: String,
    /// Hub access JWT (или `dev-{user}-token` в dev-режиме).
    pub token: String,
    /// Квалификатор вспомогательного соединения (`Some("screen")` для
    /// desktop-паблишера): даёт отдельный participant_id и не выбивает
    /// основную сессию юзера. `None` — обычное (основное) подключение.
    pub device: Option<String>,
    pub video: VideoParams,
}

/// Тайм-аут на join и на publish-ренеготиацию.
const NEGOTIATION_TIMEOUT: Duration = Duration::from_secs(15);

/// Клонируемый вход для закодированных кадров (из потока энкодера).
#[derive(Clone)]
pub struct FrameSender {
    data_tx: crossbeam::channel::Sender<DataCmd>,
}

impl FrameSender {
    /// Неблокирующая отправка. Возвращает false, если кадр дропнут
    /// (очередь engine полна — пайплайн деградировал, но не падаем).
    pub fn send(&self, frame: EncodedVideoFrame) -> bool {
        self.data_tx.try_send(DataCmd::Frame(frame)).is_ok()
    }
}

/// Клонируемый вход для Opus-пакетов (из аудио-энкодера).
#[derive(Clone)]
pub struct AudioSender {
    data_tx: crossbeam::channel::Sender<DataCmd>,
}

impl AudioSender {
    /// Неблокирующая отправка 20 мс Opus-пакета. false — дропнут под давлением.
    pub fn send(&self, frame: EncodedAudioFrame) -> bool {
        self.data_tx.try_send(DataCmd::Audio(frame)).is_ok()
    }
}

pub struct Publisher {
    ctrl_tx: crossbeam::channel::Sender<CtrlCmd>,
    data_tx: crossbeam::channel::Sender<DataCmd>,
    events_rx: mpsc::UnboundedReceiver<PublisherEvent>,
    /// События, вычитанные во время внутренних ожиданий, но адресованные
    /// потребителю — отдаются из `next_event` раньше свежих.
    pushed_back: VecDeque<PublisherEvent>,
    participant_id: Uuid,
}

impl Publisher {
    /// Подключается к сессии: WS + join (data-channel-only оффер) + ICE/DTLS.
    /// Возвращается после `Event::Connected` (транспорт готов).
    pub async fn connect(cfg: PublisherConfig) -> Result<Self, RtcClientError> {
        let url = ws_url(
            &cfg.base_url,
            &cfg.session_id,
            &cfg.token,
            cfg.device.as_deref(),
        )
        .map_err(RtcClientError::WsConnect)?;
        tracing::info!(%url, "connecting signaling WS");
        let (ws, _resp) = connect_async(&url)
            .await
            .map_err(|e| RtcClientError::WsConnect(e.to_string()))?;
        let (mut ws_sink, mut ws_stream) = ws.split();

        // UDP-сокет медиа: один на все интерфейсы; host-кандидат на каждый
        // IPv4 (тот же фильтр, что у SFU: без loopback/link-local).
        let socket = UdpSocket::bind(("0.0.0.0", 0))?;
        let port = socket.local_addr()?.port();
        let ips = local_ipv4s();
        let local_addrs: Vec<SocketAddr> =
            ips.iter().map(|ip| SocketAddr::new(*ip, port)).collect();
        // str0m адресует Receive.destination одним адресом — берём первый
        // кандидат как канонический (SFU поступает так же со своей стороны).
        let canonical_local = local_addrs
            .first()
            .copied()
            .unwrap_or_else(|| SocketAddr::new(IpAddr::from([127, 0, 0, 1]), port));

        let (core, join_offer) =
            EngineCore::new(std::time::Instant::now(), &local_addrs, cfg.video.clone())
                .map_err(RtcClientError::Sdp)?;

        let (ws_out_tx, mut ws_out_rx) = mpsc::unbounded_channel::<ClientMessage>();
        let (event_tx, events_rx) = mpsc::unbounded_channel::<PublisherEvent>();

        let channels = engine::spawn(core, socket, canonical_local, ws_out_tx.clone(), event_tx)?;

        // WS writer: engine → сокет.
        tokio::spawn(async move {
            while let Some(msg) = ws_out_rx.recv().await {
                let text = match serde_json::to_string(&msg) {
                    Ok(t) => t,
                    Err(e) => {
                        tracing::error!("serialize signaling message: {e}");
                        continue;
                    }
                };
                if ws_sink.send(Message::text(text)).await.is_err() {
                    tracing::info!("signaling WS closed (writer)");
                    return;
                }
            }
            // Publisher закрылся штатно — вежливо закрываем сокет.
            let _ = ws_sink.close().await;
        });

        // WS reader: сокет → engine. Ping/pong обрабатывает tungstenite.
        let ctrl_for_reader = channels.ctrl_tx.clone();
        tokio::spawn(async move {
            while let Some(item) = ws_stream.next().await {
                match item {
                    Ok(Message::Text(text)) => match serde_json::from_str::<ServerMessage>(&text) {
                        Ok(msg) => {
                            if ctrl_for_reader.send(CtrlCmd::Signal(msg)).is_err() {
                                return;
                            }
                        }
                        Err(e) => {
                            tracing::warn!(
                                raw = %text.chars().take(120).collect::<String>(),
                                "unparseable server message: {e}"
                            );
                        }
                    },
                    Ok(Message::Close(_)) | Err(_) => break,
                    Ok(_) => {}
                }
            }
            // WS умер — глушим engine, он дошлёт Disconnected.
            let _ = ctrl_for_reader.send(CtrlCmd::Stop);
        });

        // Join.
        ws_out_tx
            .send(ClientMessage::Join {
                sdp_offer: join_offer,
            })
            .map_err(|_| RtcClientError::EngineGone)?;

        let mut publisher = Publisher {
            ctrl_tx: channels.ctrl_tx,
            data_tx: channels.data_tx,
            events_rx,
            pushed_back: VecDeque::new(),
            participant_id: Uuid::nil(),
        };

        let connected = publisher
            .wait_for(NEGOTIATION_TIMEOUT, |e| {
                matches!(
                    e,
                    PublisherEvent::Connected { .. } | PublisherEvent::Disconnected { .. }
                )
            })
            .await?;
        match connected {
            PublisherEvent::Connected { participant_id } => {
                publisher.participant_id = participant_id;
                tracing::info!(%participant_id, "publisher connected");
                Ok(publisher)
            }
            PublisherEvent::Disconnected { reason } => Err(RtcClientError::Server(reason)),
            _ => unreachable!("wait_for predicate"),
        }
    }

    /// `publish_track` (screen video + опц. audio) + ренеготиация.
    /// Возвращается после server answer — с этого момента [`FrameSender`]
    /// (и [`AudioSender`], если `with_audio`) доставляют медиа.
    pub async fn publish_screen(&mut self, with_audio: bool) -> Result<(), RtcClientError> {
        self.ctrl_tx
            .send(CtrlCmd::PublishScreen { audio: with_audio })
            .map_err(|_| RtcClientError::EngineGone)?;
        let event = self
            .wait_for(NEGOTIATION_TIMEOUT, |e| {
                matches!(
                    e,
                    PublisherEvent::VideoPublished | PublisherEvent::Disconnected { .. }
                )
            })
            .await?;
        match event {
            PublisherEvent::VideoPublished => Ok(()),
            PublisherEvent::Disconnected { reason } => Err(RtcClientError::Server(reason)),
            _ => unreachable!("wait_for predicate"),
        }
    }

    /// Публикует микрофон (audio-only, source=camera) — нативный войс.
    /// Возвращается после server answer; далее [`AudioSender`] доставляет.
    pub async fn publish_microphone(&mut self) -> Result<(), RtcClientError> {
        self.ctrl_tx
            .send(CtrlCmd::PublishMicrophone)
            .map_err(|_| RtcClientError::EngineGone)?;
        let event = self
            .wait_for(NEGOTIATION_TIMEOUT, |e| {
                matches!(
                    e,
                    PublisherEvent::AudioPublished | PublisherEvent::Disconnected { .. }
                )
            })
            .await?;
        match event {
            PublisherEvent::AudioPublished => Ok(()),
            PublisherEvent::Disconnected { reason } => Err(RtcClientError::Server(reason)),
            _ => unreachable!("wait_for predicate"),
        }
    }

    pub fn participant_id(&self) -> Uuid {
        self.participant_id
    }

    pub fn frame_sender(&self) -> FrameSender {
        FrameSender {
            data_tx: self.data_tx.clone(),
        }
    }

    pub fn audio_sender(&self) -> AudioSender {
        AudioSender {
            data_tx: self.data_tx.clone(),
        }
    }

    /// Следующее событие (KeyframeRequested / TargetBitrate / Disconnected…).
    /// `None` — engine завершился и всё вычитано.
    pub async fn next_event(&mut self) -> Option<PublisherEvent> {
        if let Some(e) = self.pushed_back.pop_front() {
            return Some(e);
        }
        self.events_rx.recv().await
    }

    /// Штатное завершение: `leave` + остановка engine.
    pub fn close(&self) {
        let _ = self.ctrl_tx.send(CtrlCmd::Stop);
    }

    /// Ждёт событие по предикату, откладывая остальные для `next_event`.
    async fn wait_for(
        &mut self,
        timeout: Duration,
        pred: impl Fn(&PublisherEvent) -> bool,
    ) -> Result<PublisherEvent, RtcClientError> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let event = tokio::time::timeout_at(deadline, self.events_rx.recv())
                .await
                .map_err(|_| RtcClientError::Timeout("negotiation"))?
                .ok_or(RtcClientError::SignalingClosed)?;
            if pred(&event) {
                return Ok(event);
            }
            self.pushed_back.push_back(event);
        }
    }
}

impl Drop for Publisher {
    fn drop(&mut self) {
        let _ = self.ctrl_tx.send(CtrlCmd::Stop);
    }
}

/// IPv4-адреса интерфейсов — тот же фильтр, что у SFU
/// (`services/video/src/config.rs`): без loopback/link-local/unspecified.
fn local_ipv4s() -> Vec<IpAddr> {
    let Ok(ifs) = if_addrs::get_if_addrs() else {
        return vec![IpAddr::from([127, 0, 0, 1])];
    };
    let mut ips: Vec<IpAddr> = ifs
        .into_iter()
        .map(|i| i.addr.ip())
        .filter(|ip| {
            !ip.is_loopback()
                && !ip.is_unspecified()
                && match ip {
                    IpAddr::V4(v4) => !v4.is_link_local(),
                    IpAddr::V6(_) => false,
                }
        })
        .collect();
    if ips.is_empty() {
        ips.push(IpAddr::from([127, 0, 0, 1]));
    }
    ips
}

/// Создаёт (или возвращает существующую — endpoint идемпотентен по
/// `channel_id`) SFU-сессию: `POST {base}/v1/sessions`.
pub async fn ensure_session(
    base_url: &str,
    channel_id: &str,
    hub_id: &str,
) -> Result<String, RtcClientError> {
    #[derive(serde::Deserialize)]
    struct SessionResponse {
        session_id: String,
    }
    let url = format!("{}/v1/sessions", base_url.trim_end_matches('/'));
    let resp = reqwest::Client::new()
        .post(&url)
        .json(&serde_json::json!({ "channel_id": channel_id, "hub_id": hub_id }))
        .send()
        .await
        .map_err(|e| RtcClientError::SessionApi(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(RtcClientError::SessionApi(format!(
            "{url} -> HTTP {}",
            resp.status()
        )));
    }
    let body: SessionResponse = resp
        .json()
        .await
        .map_err(|e| RtcClientError::SessionApi(e.to_string()))?;
    Ok(body.session_id)
}
