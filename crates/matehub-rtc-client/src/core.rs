//! Sans-IO ядро паблишера: str0m `Rtc` + state machine нэготиации.
//!
//! Ядро не владеет ни сокетом, ни временем — все входы приходят снаружи
//! (`handle_signal` / `handle_udp` / `handle_timeout`), все выходы возвращаются
//! значениями (`CoreOut` / `CorePoll`). Это зеркалит философию str0m и делает
//! полный цикл нэготиации тестируемым в памяти без сети (см. tests ниже:
//! ядро гоняется против Rtc, сконфигурированного как наш SFU).

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use str0m::bwe::{Bitrate, BweKind};
use str0m::change::{SdpAnswer, SdpOffer, SdpPendingOffer};
use str0m::format::Codec;
use str0m::media::{Direction, Frequency, MediaKind, MediaTime, Mid, Pt};
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, IceConnectionState, Input, Output, Rtc};
use uuid::Uuid;

use crate::signaling::{ClientMessage, ServerMessage};
use crate::types::{
    EncodedAudioFrame, EncodedVideoFrame, MediaKind as PubMediaKind, PublisherEvent, VideoParams,
};

/// Что ядро просит сделать снаружи.
#[derive(Debug)]
pub(crate) enum CoreOut {
    SendSignal(ClientMessage),
    Emit(PublisherEvent),
}

/// Результат одного шага `poll`.
pub(crate) enum CorePoll {
    /// Отправить датаграмму.
    Transmit {
        destination: SocketAddr,
        contents: Vec<u8>,
    },
    /// Событий больше нет — спать до дедлайна.
    Timeout(Instant),
    /// Событие str0m переварено, продолжать poll.
    Outs(Vec<CoreOut>),
    /// Rtc мёртв, engine должен завершиться.
    Fatal(String),
}

/// Чей answer мы ждём.
#[derive(Debug, Clone, Copy, PartialEq)]
enum PendingKind {
    Join,
    Video,
    Audio,
}

pub(crate) struct EngineCore {
    rtc: Rtc,
    pending: Option<(SdpPendingOffer, PendingKind)>,
    participant_id: Option<Uuid>,
    video_mid: Option<Mid>,
    /// PT для H.264, зафиксированный после нэготиации видео m-line.
    video_pt: Option<Pt>,
    /// Audio m-line (Opus, системный звук) — `None`, пока не опубликован.
    audio_mid: Option<Mid>,
    audio_pt: Option<Pt>,
    video_params: VideoParams,
    joined: bool,
}

impl EngineCore {
    /// Собирает Rtc и join-оффер (data channel only — см. lib.rs, п.1 флоу).
    ///
    /// `local_addrs` — адреса host-кандидатов (наш UDP-порт на каждом
    /// интерфейсе). Кандидаты кладутся в оффер целиком, trickle не нужен.
    pub(crate) fn new(
        now: Instant,
        local_addrs: &[SocketAddr],
        video_params: VideoParams,
    ) -> Result<(Self, String), String> {
        let mut rtc = Rtc::builder()
            .clear_codecs()
            .enable_h264(true)
            // Opus обязателен, даже пока мы не шлём аудио: SFU офферит
            // audio m-line'ы чужих микрофонов, и answer без единого PT
            // невалиден (ловится loopback-тестом glare-сценария).
            .enable_opus(true)
            .enable_bwe(Some(Bitrate::bps(video_params.target_bitrate_bps)))
            .build(now);

        for addr in local_addrs {
            let candidate =
                Candidate::host(*addr, "udp").map_err(|e| format!("host candidate {addr}: {e}"))?;
            rtc.add_local_candidate(candidate);
        }

        let mut api = rtc.sdp_api();
        api.add_channel("matehub-ctrl".to_string());
        let (offer, pending) = api
            .apply()
            .ok_or_else(|| "join offer produced no changes".to_string())?;

        let core = Self {
            rtc,
            pending: Some((pending, PendingKind::Join)),
            participant_id: None,
            video_mid: None,
            video_pt: None,
            audio_mid: None,
            audio_pt: None,
            video_params,
            joined: false,
        };
        Ok((core, offer.to_sdp_string()))
    }

    pub(crate) fn is_alive(&self) -> bool {
        self.rtc.is_alive()
    }

    /// `publish_track`-хинты + ренеготиация с sendonly video (и, при
    /// `with_audio`, audio) m-line'ами в одном оффере. Хинты идут ДО оффера
    /// (SFU матчит их FIFO по kind на `MediaAdded`).
    pub(crate) fn publish_screen(&mut self, with_audio: bool) -> Vec<CoreOut> {
        if !self.joined {
            return vec![CoreOut::Emit(PublisherEvent::ServerError(
                "publish_screen before join completed".into(),
            ))];
        }
        if self.video_mid.is_some() || matches!(self.pending, Some((_, PendingKind::Video))) {
            // Идемпотентность: повторный вызов не плодит m-line'ы.
            return Vec::new();
        }

        let mut outs = vec![CoreOut::SendSignal(ClientMessage::PublishTrack {
            source: "screen".into(),
            kind: "video".into(),
            track_id: None,
        })];
        if with_audio {
            outs.push(CoreOut::SendSignal(ClientMessage::PublishTrack {
                source: "screen".into(),
                kind: "audio".into(),
                track_id: None,
            }));
        }

        let mut api = self.rtc.sdp_api();
        let vmid = api.add_media(MediaKind::Video, Direction::SendOnly, None, None, None);
        let amid = with_audio
            .then(|| api.add_media(MediaKind::Audio, Direction::SendOnly, None, None, None));
        let Some((offer, pending)) = api.apply() else {
            outs.push(CoreOut::Emit(PublisherEvent::ServerError(
                "screen offer produced no changes".into(),
            )));
            return outs;
        };
        self.video_mid = Some(vmid);
        self.audio_mid = amid;
        self.pending = Some((pending, PendingKind::Video));
        outs.push(CoreOut::SendSignal(ClientMessage::Offer {
            sdp_offer: offer.to_sdp_string(),
        }));
        outs
    }

    /// Публикует ТОЛЬКО аудио-трек микрофона (нативный войс). `source=camera`
    /// — так браузер помечает микрофон; SFU кладёт его в общий аудио-микс.
    pub(crate) fn publish_microphone(&mut self) -> Vec<CoreOut> {
        if !self.joined {
            return vec![CoreOut::Emit(PublisherEvent::ServerError(
                "publish_microphone before join completed".into(),
            ))];
        }
        if self.audio_mid.is_some() || matches!(self.pending, Some((_, PendingKind::Audio))) {
            return Vec::new();
        }
        let mut outs = vec![CoreOut::SendSignal(ClientMessage::PublishTrack {
            source: "camera".into(),
            kind: "audio".into(),
            track_id: None,
        })];
        let mut api = self.rtc.sdp_api();
        let amid = api.add_media(MediaKind::Audio, Direction::SendOnly, None, None, None);
        let Some((offer, pending)) = api.apply() else {
            outs.push(CoreOut::Emit(PublisherEvent::ServerError(
                "microphone offer produced no changes".into(),
            )));
            return outs;
        };
        self.audio_mid = Some(amid);
        self.pending = Some((pending, PendingKind::Audio));
        outs.push(CoreOut::SendSignal(ClientMessage::Offer {
            sdp_offer: offer.to_sdp_string(),
        }));
        outs
    }

    pub(crate) fn handle_signal(&mut self, msg: ServerMessage) -> Vec<CoreOut> {
        match msg {
            ServerMessage::Answer {
                sdp_answer,
                participant_id,
            } => self.on_answer(&sdp_answer, participant_id),
            ServerMessage::Offer { sdp_offer, .. } => self.on_server_offer(&sdp_offer),
            ServerMessage::IceCandidate { candidate, .. } => {
                // Сервер сегодня не триклит (кандидаты в answer), но протокол
                // это допускает — поддерживаем на будущее.
                match Candidate::from_sdp_string(&candidate) {
                    Ok(c) => self.rtc.add_remote_candidate(c),
                    Err(e) => {
                        tracing::warn!(%candidate, "unparseable remote candidate: {e}");
                    }
                }
                Vec::new()
            }
            ServerMessage::Error { message } => {
                vec![CoreOut::Emit(PublisherEvent::ServerError(message))]
            }
            ServerMessage::ForceDisconnected { reason } => {
                self.rtc.disconnect();
                vec![CoreOut::Emit(PublisherEvent::Disconnected { reason })]
            }
            // Ростер — наверх (нужен нативному войсу для UI и корреляции
            // mid↔участник). Мьют/дифен других — UI-only, пропускаем.
            ServerMessage::ParticipantJoined {
                participant_id,
                user_id,
            } => vec![CoreOut::Emit(PublisherEvent::ParticipantJoined {
                participant_id,
                user_id,
            })],
            ServerMessage::ParticipantLeft {
                participant_id,
                user_id,
            } => vec![CoreOut::Emit(PublisherEvent::ParticipantLeft {
                participant_id,
                user_id,
            })],
            ServerMessage::ParticipantMuted { .. } | ServerMessage::ParticipantDeafened { .. } => {
                Vec::new()
            }
        }
    }

    fn on_answer(&mut self, sdp_answer: &str, participant_id: Uuid) -> Vec<CoreOut> {
        let answer = match SdpAnswer::from_sdp_string(sdp_answer) {
            Ok(a) => a,
            Err(e) => {
                return vec![CoreOut::Emit(PublisherEvent::ServerError(format!(
                    "unparseable SDP answer: {e}"
                )))];
            }
        };
        let Some((pending, kind)) = self.pending.take() else {
            tracing::warn!("SDP answer with no pending offer — dropping");
            return Vec::new();
        };
        if let Err(e) = self.rtc.sdp_api().accept_answer(pending, answer) {
            return vec![CoreOut::Emit(PublisherEvent::ServerError(format!(
                "accept_answer failed: {e}"
            )))];
        }
        match kind {
            PendingKind::Join => {
                self.joined = true;
                self.participant_id = Some(participant_id);
                Vec::new()
            }
            PendingKind::Video => {
                // PT фиксируем лениво в write_frame: сразу после answer
                // media может быть ещё не полностью готова.
                vec![
                    CoreOut::Emit(PublisherEvent::VideoPublished),
                    // Пусть тайлы подписчиков сразу видят активный шаринг.
                    CoreOut::SendSignal(ClientMessage::MuteChanged {
                        kind: "video".into(),
                        muted: false,
                    }),
                ]
            }
            PendingKind::Audio => {
                vec![
                    CoreOut::Emit(PublisherEvent::AudioPublished),
                    CoreOut::SendSignal(ClientMessage::MuteChanged {
                        kind: "audio".into(),
                        muted: false,
                    }),
                ]
            }
        }
    }

    /// Серверный оффер (подписки на чужие треки). Мы обязаны ответить,
    /// иначе нэготиация встаёт. Glare-протокол — см. lib.rs.
    fn on_server_offer(&mut self, sdp_offer: &str) -> Vec<CoreOut> {
        let offer = match SdpOffer::from_sdp_string(sdp_offer) {
            Ok(o) => o,
            Err(e) => {
                return vec![CoreOut::Emit(PublisherEvent::ServerError(format!(
                    "unparseable server offer: {e}"
                )))];
            }
        };
        // accept_offer инвалидирует pending — забираем его ДО вызова,
        // чтобы воскресить изменения после ответа.
        let inflight = self.pending.take();

        let answer = match self.rtc.sdp_api().accept_offer(offer) {
            Ok(a) => a,
            Err(e) => {
                return vec![CoreOut::Emit(PublisherEvent::ServerError(format!(
                    "accept_offer failed: {e}"
                )))];
            }
        };
        let mut outs = vec![CoreOut::SendSignal(ClientMessage::Answer {
            sdp_answer: answer.to_sdp_string(),
        })];

        if let Some((old_pending, kind)) = inflight {
            // Наш оффер застрял на сервере в queued_client_offer, но он
            // СТАРЕЕ серверного: если серверный оффер добавил m-line, SFU
            // при дрейне ответит "Changed order for m-line" и молча дропнет
            // его (handle_client_offer: warn + return None, ни Error, ни
            // Answer). Поэтому: merge() воскрешает изменения с теми же mid,
            // но офер пересобирается под НОВОЕ состояние сессии — и уходит
            // повторно. Порядок на сокете: Answer, затем свежий Offer —
            // сервер сначала дренирует (и дропнет) старый, затем штатно
            // ответит на новый.
            let mut api = self.rtc.sdp_api();
            api.merge(old_pending);
            match api.apply() {
                Some((offer, pending)) => {
                    self.pending = Some((pending, kind));
                    outs.push(CoreOut::SendSignal(ClientMessage::Offer {
                        sdp_offer: offer.to_sdp_string(),
                    }));
                }
                None => {
                    outs.push(CoreOut::Emit(PublisherEvent::ServerError(
                        "failed to resurrect in-flight offer after glare".into(),
                    )));
                }
            }
        }
        outs
    }

    /// Записать кадр. Возвращает false, если видео ещё не нэготиировано
    /// (кадр дропнут — это нормальный transient при старте).
    pub(crate) fn write_frame(&mut self, frame: &EncodedVideoFrame) -> bool {
        let Some(mid) = self.video_mid else {
            return false;
        };
        if self.pending.is_some() && self.video_pt.is_none() {
            // Ренеготиация ещё в полёте.
            return false;
        }
        if self.video_pt.is_none() {
            let Some(writer) = self.rtc.writer(mid) else {
                return false;
            };
            let Some(pt) = writer
                .payload_params()
                .find(|p| p.spec().codec == Codec::H264)
                .map(|p| p.pt())
            else {
                tracing::error!("no H264 payload type negotiated");
                return false;
            };
            self.video_pt = Some(pt);
        }
        let pt = self.video_pt.expect("set above");
        let Some(writer) = self.rtc.writer(mid) else {
            return false;
        };
        let rtp_time = MediaTime::new(frame.rtp_time_90khz, Frequency::NINETY_KHZ);
        match writer.write(pt, frame.captured_at, rtp_time, Arc::clone(&frame.data)) {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!("frame write failed: {e}");
                false
            }
        }
    }

    /// Записать Opus-пакет (20 мс). `audio_level` (RFC 6464) стамповка — то,
    /// что читает top-K селектор SFU. Возвращает false, если аудио ещё не
    /// нэготиировано (transient на старте).
    pub(crate) fn write_audio(&mut self, frame: &EncodedAudioFrame) -> bool {
        let Some(mid) = self.audio_mid else {
            return false;
        };
        if self.pending.is_some() && self.audio_pt.is_none() {
            return false;
        }
        if self.audio_pt.is_none() {
            let Some(writer) = self.rtc.writer(mid) else {
                return false;
            };
            let Some(pt) = writer
                .payload_params()
                .find(|p| p.spec().codec == Codec::Opus)
                .map(|p| p.pt())
            else {
                tracing::error!("no Opus payload type negotiated");
                return false;
            };
            self.audio_pt = Some(pt);
        }
        let pt = self.audio_pt.expect("set above");
        let Some(writer) = self.rtc.writer(mid) else {
            return false;
        };
        let rtp_time = MediaTime::new(frame.rtp_time_48khz, Frequency::FORTY_EIGHT_KHZ);
        // str0m хочет уровень в отрицательных dB (0 = максимум); наш dBov —
        // 0..127 в стиле RFC 6464 (0 = громко). Инвертируем.
        let level = -(frame.audio_level_dbov.min(127) as i8);
        match writer.audio_level(level, frame.voice).write(
            pt,
            frame.captured_at,
            rtp_time,
            Arc::clone(&frame.data),
        ) {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!("audio write failed: {e}");
                false
            }
        }
    }

    pub(crate) fn handle_udp(
        &mut self,
        now: Instant,
        source: SocketAddr,
        local: SocketAddr,
        buf: &[u8],
    ) {
        let Ok(contents) = buf.try_into() else {
            tracing::trace!("non-RTC datagram from {source}");
            return;
        };
        let input = Input::Receive(
            now,
            Receive {
                proto: Protocol::Udp,
                source,
                destination: local,
                contents,
            },
        );
        if let Err(e) = self.rtc.handle_input(input) {
            tracing::warn!("rtc input error: {e}");
        }
    }

    pub(crate) fn handle_timeout(&mut self, now: Instant) {
        if let Err(e) = self.rtc.handle_input(Input::Timeout(now)) {
            tracing::warn!("rtc timeout error: {e}");
        }
    }

    pub(crate) fn poll(&mut self) -> CorePoll {
        match self.rtc.poll_output() {
            Ok(Output::Transmit(t)) => CorePoll::Transmit {
                destination: t.destination,
                contents: t.contents.to_vec(),
            },
            Ok(Output::Timeout(deadline)) => CorePoll::Timeout(deadline),
            Ok(Output::Event(event)) => CorePoll::Outs(self.handle_event(event)),
            Err(e) => CorePoll::Fatal(e.to_string()),
        }
    }

    fn handle_event(&mut self, event: Event) -> Vec<CoreOut> {
        match event {
            Event::Connected => {
                // Пейсер должен пробить полосу выше текущего send-rate,
                // иначе BWE-оценка не вырастет до целевого битрейта.
                self.rtc
                    .bwe()
                    .set_desired_bitrate(Bitrate::bps(self.video_params.target_bitrate_bps));
                let participant_id = self.participant_id.unwrap_or_else(Uuid::nil);
                vec![CoreOut::Emit(PublisherEvent::Connected { participant_id })]
            }
            Event::IceConnectionStateChange(IceConnectionState::Disconnected) => {
                vec![CoreOut::Emit(PublisherEvent::Disconnected {
                    reason: "ice disconnected".into(),
                })]
            }
            Event::KeyframeRequest(_) => {
                // v1 — один слой, rid в запросе не различаем.
                vec![CoreOut::Emit(PublisherEvent::KeyframeRequested)]
            }
            // BweKind — #[non_exhaustive], поэтому отдельный биндинг с
            // wildcard-веткой внутри общего match невозможен без collapse.
            Event::EgressBitrateEstimate(BweKind::Twcc(bitrate))
            | Event::EgressBitrateEstimate(BweKind::Remb(_, bitrate)) => {
                vec![CoreOut::Emit(PublisherEvent::TargetBitrate(
                    bitrate.as_u64(),
                ))]
            }
            Event::MediaAdded(m) => {
                let kind = match m.kind {
                    MediaKind::Audio => PubMediaKind::Audio,
                    MediaKind::Video => PubMediaKind::Video,
                };
                vec![CoreOut::Emit(PublisherEvent::RemoteTrackAdded {
                    mid: m.mid.to_string(),
                    kind,
                })]
            }
            // Входящий Opus чужого микрофона (sample mode: data — цельный
            // депэйлоаженный Opus-пакет). Видео игнорируем: нативный клиент
            // звук воспроизводит сам, видео рисует webview-UI.
            Event::MediaData(d) if d.params.spec().codec == Codec::Opus => {
                vec![CoreOut::Emit(PublisherEvent::RemoteAudio {
                    mid: d.mid.to_string(),
                    data: d.data,
                })]
            }
            // Каналы, статистика, входящее видео — не нужны.
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
#[path = "core_tests.rs"]
mod tests;
