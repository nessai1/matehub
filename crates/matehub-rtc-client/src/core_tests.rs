//! In-memory loopback: EngineCore против Rtc, сконфигурированного как наш
//! SFU (`set_rtp_mode(true)`, full ICE, host-кандидаты). Датаграммы
//! перекладываются вручную, время виртуальное — сеть не нужна.
//!
//! Проверяем контракт, который невозможно поймать юнит-тестами по кускам:
//! join (data-channel-only) → ICE/DTLS → publish-ренеготиация → H.264 кадры
//! доезжают до сервера как `Event::RtpPacket`, включая glare-сценарий.

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use str0m::change::{SdpAnswer, SdpOffer};
use str0m::ice::IceCreds;
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, Input, Output, Rtc};
use uuid::Uuid;

use super::{CoreOut, CorePoll, EngineCore};
use crate::signaling::{ClientMessage, ServerMessage};
use crate::types::{EncodedAudioFrame, EncodedVideoFrame, PublisherEvent, VideoParams};

const CLIENT_ADDR: &str = "127.0.0.1:6001";
const SERVER_ADDR: &str = "127.0.0.1:4001";

struct Loopback {
    core: EngineCore,
    server: Rtc,
    now: Instant,
    client_addr: SocketAddr,
    server_addr: SocketAddr,
    client_outs: Vec<CoreOut>,
    server_events: Vec<Event>,
}

impl Loopback {
    /// Собирает пару и проводит join-хендшейк на уровне SDP (без ICE).
    fn new() -> Self {
        let now = Instant::now();
        let client_addr: SocketAddr = CLIENT_ADDR.parse().unwrap();
        let server_addr: SocketAddr = SERVER_ADDR.parse().unwrap();

        let (core, join_offer) =
            EngineCore::new(now, &[client_addr], VideoParams::default()).unwrap();

        // Зеркало services/video/src/sfu/mod.rs::handle_join.
        let mut server = Rtc::builder()
            .set_local_ice_credentials(IceCreds::new())
            .set_rtp_mode(true)
            .build(now);
        server.add_local_candidate(Candidate::host(server_addr, "udp").unwrap());

        let offer = SdpOffer::from_sdp_string(&join_offer).unwrap();
        let answer = server.sdp_api().accept_offer(offer).unwrap();

        let mut lb = Self {
            core,
            server,
            now,
            client_addr,
            server_addr,
            client_outs: Vec::new(),
            server_events: Vec::new(),
        };
        lb.signal(ServerMessage::Answer {
            sdp_answer: answer.to_sdp_string(),
            participant_id: Uuid::new_v4(),
        });
        lb
    }

    fn signal(&mut self, msg: ServerMessage) {
        let outs = self.core.handle_signal(msg);
        self.client_outs.extend(outs);
    }

    /// Гоняет обе стороны до квиесценции или выполнения предиката.
    /// Инвариант str0m «drain после каждой мутации» соблюдается: за одну
    /// итерацию каждой стороне доставляется не больше одной датаграммы,
    /// затем обе стороны выкачиваются досуха.
    fn pump_until(&mut self, label: &str, mut done: impl FnMut(&Self) -> bool) {
        let mut to_server: VecDeque<Vec<u8>> = VecDeque::new();
        let mut to_client: VecDeque<Vec<u8>> = VecDeque::new();

        for _ in 0..10_000 {
            if done(self) {
                return;
            }

            // Drain клиента.
            let client_deadline = loop {
                match self.core.poll() {
                    CorePoll::Transmit { contents, .. } => to_server.push_back(contents),
                    CorePoll::Outs(outs) => self.client_outs.extend(outs),
                    CorePoll::Timeout(t) => break t,
                    CorePoll::Fatal(e) => panic!("client rtc died during '{label}': {e}"),
                }
            };
            // Drain сервера.
            let server_deadline = loop {
                match self.server.poll_output().unwrap() {
                    Output::Transmit(t) => to_client.push_back(t.contents.to_vec()),
                    Output::Event(e) => self.server_events.push(e),
                    Output::Timeout(t) => break t,
                }
            };

            // По одной датаграмме на сторону за итерацию.
            let mut delivered = false;
            if let Some(buf) = to_server.pop_front() {
                let receive = Receive::new(Protocol::Udp, self.client_addr, self.server_addr, &buf)
                    .expect("parseable datagram");
                self.server
                    .handle_input(Input::Receive(self.now, receive))
                    .unwrap();
                delivered = true;
            }
            if let Some(buf) = to_client.pop_front() {
                self.core
                    .handle_udp(self.now, self.server_addr, self.client_addr, &buf);
                delivered = true;
            }
            if delivered {
                continue;
            }

            // Квиесценция: прыгаем к ближайшему дедлайну.
            let next = client_deadline.min(server_deadline);
            self.now = next.max(self.now + Duration::from_millis(1));
            self.core.handle_timeout(self.now);
            self.server.handle_input(Input::Timeout(self.now)).unwrap();
        }
        panic!("pump_until('{label}') did not converge in 10k iterations");
    }

    fn client_connected(&self) -> bool {
        self.client_outs
            .iter()
            .any(|o| matches!(o, CoreOut::Emit(PublisherEvent::Connected { .. })))
    }

    fn take_signals(&mut self) -> Vec<ClientMessage> {
        let mut msgs = Vec::new();
        self.client_outs.retain(|o| match o {
            CoreOut::SendSignal(m) => {
                msgs.push(m.clone());
                false
            }
            _ => true,
        });
        msgs
    }

    fn server_rtp_packets(&self) -> usize {
        self.server_events
            .iter()
            .filter(|e| matches!(e, Event::RtpPacket(_)))
            .count()
    }

    fn establish(&mut self) {
        self.pump_until("ice+dtls connect", |lb| lb.client_connected());
    }

    /// Полный publish-флоу без glare: outs → сервер, answer → клиент.
    fn negotiate_video(&mut self) {
        self.negotiate_screen(false);
    }

    fn negotiate_screen(&mut self, with_audio: bool) {
        let outs = self.core.publish_screen(with_audio);
        self.client_outs.extend(outs);
        let msgs = self.take_signals();
        assert!(
            matches!(
                msgs.first(),
                Some(ClientMessage::PublishTrack { source, kind, .. })
                    if source == "screen" && kind == "video"
            ),
            "publish_track(video) must precede the offer, got: {msgs:?}"
        );
        // Оффер идёт последним; между ним и video-хинтом — audio-хинт (если есть).
        let Some(ClientMessage::Offer { sdp_offer }) = msgs.last() else {
            panic!("expected renegotiation offer last, got: {msgs:?}");
        };
        if with_audio {
            assert!(
                msgs.iter().any(|m| matches!(
                    m,
                    ClientMessage::PublishTrack { source, kind, .. }
                        if source == "screen" && kind == "audio"
                )),
                "audio publish_track hint missing, got: {msgs:?}"
            );
        }

        let offer = SdpOffer::from_sdp_string(sdp_offer).unwrap();
        let answer = self.server.sdp_api().accept_offer(offer).unwrap();
        self.signal(ServerMessage::Answer {
            sdp_answer: answer.to_sdp_string(),
            participant_id: Uuid::new_v4(),
        });

        assert!(
            self.client_outs
                .iter()
                .any(|o| matches!(o, CoreOut::Emit(PublisherEvent::VideoPublished))),
            "VideoPublished must be emitted after the answer"
        );
    }

    fn server_media_kinds(&self) -> Vec<str0m::media::MediaKind> {
        self.server_events
            .iter()
            .filter_map(|e| match e {
                Event::MediaAdded(m) => Some(m.kind),
                _ => None,
            })
            .collect()
    }

    fn send_fake_frames(&mut self, count: usize) {
        for i in 0..count {
            let frame = EncodedVideoFrame {
                data: fake_h264_idr_au(),
                rtp_time_90khz: (i as u64) * 1500, // 60 fps
                captured_at: self.now,
                keyframe: true,
            };
            assert!(
                self.core.write_frame(&frame),
                "frame {i} rejected after VideoPublished"
            );
            // Drain после мутации + прогон сети.
            self.pump_until("frame delivery", |lb| lb.server_rtp_packets() > i);
        }
    }
}

/// Минимальный синтетический access unit: SPS + PPS + IDR. Пакетизатору
/// str0m нужны только валидные start codes и NAL-заголовки.
fn fake_h264_idr_au() -> Arc<[u8]> {
    let mut v: Vec<u8> = Vec::new();
    v.extend_from_slice(&[0, 0, 0, 1, 0x67, 0x42, 0xc0, 0x1f, 0x8c, 0x8d, 0x40]);
    v.extend_from_slice(&[0, 0, 0, 1, 0x68, 0xce, 0x3c, 0x80]);
    v.extend_from_slice(&[0, 0, 0, 1, 0x65, 0x88, 0x84, 0x00]);
    v.extend(std::iter::repeat_n(0xAB, 400));
    v.into()
}

#[test]
fn join_with_data_channel_connects() {
    let mut lb = Loopback::new();
    lb.establish();
    assert!(lb.client_connected());
    // Data-channel-only join не должен породить медиа-треков на сервере.
    assert!(
        !lb.server_events
            .iter()
            .any(|e| matches!(e, Event::MediaAdded(_))),
        "join offer must not create media tracks"
    );
}

#[test]
fn publish_video_delivers_rtp_to_rtp_mode_server() {
    let mut lb = Loopback::new();
    lb.establish();
    lb.negotiate_video();

    // Сервер должен увидеть MediaAdded(video) после ренеготиации.
    lb.pump_until("media added", |lb| {
        lb.server_events
            .iter()
            .any(|e| matches!(e, Event::MediaAdded(_)))
    });

    lb.send_fake_frames(3);
    assert!(
        lb.server_rtp_packets() >= 3,
        "expected >=3 RTP packets, got {}",
        lb.server_rtp_packets()
    );
}

#[test]
fn publish_screen_with_audio_adds_opus_track_and_delivers() {
    let mut lb = Loopback::new();
    lb.establish();
    lb.negotiate_screen(true);

    // Сервер должен увидеть и video, и audio m-line.
    lb.pump_until("both media added", |lb| {
        let kinds = lb.server_media_kinds();
        kinds.contains(&str0m::media::MediaKind::Video)
            && kinds.contains(&str0m::media::MediaKind::Audio)
    });

    // Один 20мс Opus-кадр (синтетический payload — str0m Opus-пакетайзер
    // passthrough, ему нужны только байты).
    let audio = EncodedAudioFrame {
        data: vec![0xFC; 80].into(),
        rtp_time_48khz: 0,
        captured_at: lb.now,
        audio_level_dbov: 20,
        voice: true,
    };
    assert!(
        lb.core.write_audio(&audio),
        "audio frame rejected after publish"
    );
    let before = lb.server_rtp_packets();
    lb.pump_until("audio rtp", |lb| lb.server_rtp_packets() > before);
}

#[test]
fn publish_microphone_delivers_opus() {
    let mut lb = Loopback::new();
    lb.establish();

    let outs = lb.core.publish_microphone();
    lb.client_outs.extend(outs);
    let msgs = lb.take_signals();
    assert!(
        msgs.iter().any(|m| matches!(
            m,
            ClientMessage::PublishTrack { source, kind, .. }
                if source == "camera" && kind == "audio"
        )),
        "mic publish_track(camera/audio) missing: {msgs:?}"
    );
    let Some(ClientMessage::Offer { sdp_offer }) = msgs.last() else {
        panic!("expected mic offer last, got {msgs:?}");
    };
    let offer = SdpOffer::from_sdp_string(sdp_offer).unwrap();
    let answer = lb.server.sdp_api().accept_offer(offer).unwrap();
    lb.signal(ServerMessage::Answer {
        sdp_answer: answer.to_sdp_string(),
        participant_id: Uuid::new_v4(),
    });
    assert!(
        lb.client_outs
            .iter()
            .any(|o| matches!(o, CoreOut::Emit(PublisherEvent::AudioPublished))),
        "AudioPublished must fire after mic answer"
    );

    lb.pump_until("mic media added", |lb| {
        lb.server_media_kinds()
            .contains(&str0m::media::MediaKind::Audio)
    });
    let audio = EncodedAudioFrame {
        data: vec![0xFC; 80].into(),
        rtp_time_48khz: 0,
        captured_at: lb.now,
        audio_level_dbov: 10,
        voice: true,
    };
    assert!(lb.core.write_audio(&audio));
    let before = lb.server_rtp_packets();
    lb.pump_until("mic rtp", |lb| lb.server_rtp_packets() > before);
}

#[test]
fn server_offer_with_audio_yields_remote_track() {
    let mut lb = Loopback::new();
    lb.establish();

    // Сервер (SFU) подписывает клиента на чужой микрофон: sendonly audio.
    let mut api = lb.server.sdp_api();
    api.add_media(
        str0m::media::MediaKind::Audio,
        str0m::media::Direction::SendOnly,
        None,
        None,
        None,
    );
    let (offer, pending) = api.apply().unwrap();
    lb.signal(ServerMessage::Offer {
        sdp_offer: offer.to_sdp_string(),
        tracks: None,
    });
    // Клиент отвечает; забираем answer на сервер, чтобы завершить нэготиацию.
    let msgs = lb.take_signals();
    let Some(ClientMessage::Answer { sdp_answer }) = msgs.first() else {
        panic!("client must answer server offer, got {msgs:?}");
    };
    let answer = SdpAnswer::from_sdp_string(sdp_answer).unwrap();
    lb.server.sdp_api().accept_answer(pending, answer).unwrap();

    // Клиентский Rtc должен поднять входящий трек → RemoteTrackAdded(Audio).
    lb.pump_until("remote track added", |lb| {
        lb.client_outs.iter().any(|o| {
            matches!(
                o,
                CoreOut::Emit(PublisherEvent::RemoteTrackAdded {
                    kind: crate::types::MediaKind::Audio,
                    ..
                })
            )
        })
    });
}

#[test]
fn participant_roster_events_surface() {
    let mut lb = Loopback::new();
    lb.establish();
    let pid = Uuid::new_v4();
    lb.signal(ServerMessage::ParticipantJoined {
        participant_id: pid,
        user_id: "bob".into(),
    });
    lb.signal(ServerMessage::ParticipantLeft {
        participant_id: pid,
        user_id: "bob".into(),
    });
    assert!(lb.client_outs.iter().any(|o| matches!(
        o,
        CoreOut::Emit(PublisherEvent::ParticipantJoined { user_id, .. }) if user_id == "bob"
    )));
    assert!(lb.client_outs.iter().any(|o| matches!(
        o,
        CoreOut::Emit(PublisherEvent::ParticipantLeft { user_id, .. }) if user_id == "bob"
    )));
}

#[test]
fn audio_before_negotiation_is_rejected_not_fatal() {
    let mut lb = Loopback::new();
    lb.establish();
    let audio = EncodedAudioFrame {
        data: vec![0xFC; 80].into(),
        rtp_time_48khz: 0,
        captured_at: lb.now,
        audio_level_dbov: 0,
        voice: false,
    };
    // Аудио не публиковали — write_audio должен вернуть false, не паникуя.
    assert!(!lb.core.write_audio(&audio));
    assert!(lb.core.is_alive());
}

#[test]
fn frames_before_negotiation_are_rejected_not_fatal() {
    let mut lb = Loopback::new();
    lb.establish();
    let frame = EncodedVideoFrame {
        data: fake_h264_idr_au(),
        rtp_time_90khz: 0,
        captured_at: lb.now,
        keyframe: true,
    };
    assert!(!lb.core.write_frame(&frame));
    assert!(lb.core.is_alive());
}

#[test]
fn glare_resurrects_inflight_publish_offer() {
    let mut lb = Loopback::new();
    lb.establish();

    // Наш publish-оффер уходит «в полёт»…
    let outs = lb.core.publish_screen(false);
    lb.client_outs.extend(outs);
    let msgs = lb.take_signals();
    let Some(ClientMessage::Offer {
        sdp_offer: inflight_offer,
    }) = msgs.get(1).cloned()
    else {
        panic!("expected offer, got {msgs:?}");
    };

    // …а сервер в этот момент шлёт свой: SFU queue'ит наш оффер
    // (sfu/mod.rs::handle_client_offer) и дренирует его после нашего answer.
    let mut api = lb.server.sdp_api();
    api.add_media(
        str0m::media::MediaKind::Audio,
        str0m::media::Direction::SendOnly,
        None,
        None,
        None,
    );
    let (server_offer, server_pending) = api.apply().unwrap();
    lb.signal(ServerMessage::Offer {
        sdp_offer: server_offer.to_sdp_string(),
        tracks: None,
    });

    // Клиент обязан ответить на серверный оффер И переотправить свой,
    // пересобранный под новое состояние сессии (с теми же mid).
    let msgs = lb.take_signals();
    let Some(ClientMessage::Answer { sdp_answer }) = msgs.first() else {
        panic!("client must answer the server offer during glare, got {msgs:?}");
    };
    let Some(ClientMessage::Offer { sdp_offer: reoffer }) = msgs.get(1).cloned() else {
        panic!("client must re-send the in-flight offer after glare, got {msgs:?}");
    };
    let answer = SdpAnswer::from_sdp_string(sdp_answer).unwrap();
    lb.server
        .sdp_api()
        .accept_answer(server_pending, answer)
        .unwrap();

    // Сервер дренирует СТАРЫЙ отложенный оффер — тот устарел (серверный
    // оффер добавил m-line) и обязан быть отвергнут. Ровно так ведёт себя
    // SFU: warn + молчаливый дроп, без Answer и без Error.
    let stale = SdpOffer::from_sdp_string(&inflight_offer).unwrap();
    assert!(
        lb.server.sdp_api().accept_offer(stale).is_err(),
        "stale queued offer must be rejected — this is why the client re-offers"
    );

    // Затем сервер штатно принимает свежий переотправленный оффер.
    let offer = SdpOffer::from_sdp_string(&reoffer).unwrap();
    let late_answer = lb.server.sdp_api().accept_offer(offer).unwrap();
    lb.signal(ServerMessage::Answer {
        sdp_answer: late_answer.to_sdp_string(),
        participant_id: Uuid::new_v4(),
    });

    assert!(
        lb.client_outs
            .iter()
            .any(|o| matches!(o, CoreOut::Emit(PublisherEvent::VideoPublished))),
        "resurrected pending must accept the late answer"
    );

    // И медиа после всего этого реально ходит.
    lb.pump_until("post-glare media added", |lb| {
        lb.server_events
            .iter()
            .any(|e| matches!(e, Event::MediaAdded(_)))
    });
    lb.send_fake_frames(2);
    assert!(lb.server_rtp_packets() >= 2);
}

#[test]
fn force_disconnect_kills_rtc() {
    let mut lb = Loopback::new();
    lb.establish();
    lb.signal(ServerMessage::ForceDisconnected {
        reason: "joined_elsewhere".into(),
    });
    assert!(
        lb.client_outs
            .iter()
            .any(|o| matches!(o, CoreOut::Emit(PublisherEvent::Disconnected { .. })))
    );
    assert!(!lb.core.is_alive());
}
