use std::sync::Arc;
use std::time::Instant;

use uuid::Uuid;

/// Закодированный видеокадр — один access unit H.264 в Annex B
/// (start codes `00 00 00 01`; для IDR перед ним SPS+PPS — str0m
/// закеширует их и выдаст STAP-A, см. RFC 6184 pkt-mode 1).
#[derive(Debug, Clone)]
pub struct EncodedVideoFrame {
    pub data: Arc<[u8]>,
    /// RTP-время кадра в 90 kHz тиках (монотонное, от момента захвата).
    pub rtp_time_90khz: u64,
    /// Момент захвата кадра — wallclock для str0m (SR-синхронизация).
    pub captured_at: Instant,
    /// Информационно: кадр — IDR. На трансляцию не влияет, полезно для логов.
    pub keyframe: bool,
}

/// Закодированный аудиокадр — один Opus-пакет (20 мс, 48 kHz stereo).
/// str0m в sample mode пишет его как payload без обёртки (Opus-пакетайзер —
/// passthrough), см. `docs/video/desktop-share.md`.
#[derive(Debug, Clone)]
pub struct EncodedAudioFrame {
    pub data: Arc<[u8]>,
    /// RTP-время в 48 kHz тиках: +960 на каждый 20 мс кадр.
    pub rtp_time_48khz: u64,
    /// Момент захвата — общий wallclock-base с видео (лип-синк через RTCP SR).
    pub captured_at: Instant,
    /// Громкость в -dBov (0 = максимум, ~-127 = тишина) для RFC 6464 —
    /// это поле читает top-K селектор SFU.
    pub audio_level_dbov: u8,
    /// Есть речь (voice activity) — во ту же расширение.
    pub voice: bool,
}

/// Параметры видеопаблиша. v1 — один слой без simulcast: SFU-гейт форвардит
/// no-rid поток всегда, а слой `l` дропает на ингресте (жечь аплинк незачем).
#[derive(Debug, Clone)]
pub struct VideoParams {
    /// Стартовая оценка BWE и желаемый битрейт пейсера, bps.
    pub target_bitrate_bps: u64,
}

impl Default for VideoParams {
    fn default() -> Self {
        Self {
            // Профиль "gaming" браузерного пути: 1080p60 ≈ 6 Mbps.
            target_bitrate_bps: 6_000_000,
        }
    }
}

/// События паблишера/клиента наверх (в приложение).
#[derive(Debug, Clone)]
pub enum PublisherEvent {
    /// ICE+DTLS установлены, медиа можно писать после `VideoPublished`.
    Connected { participant_id: Uuid },
    /// Ренеготиация видео завершена — кадры пойдут в сеть.
    VideoPublished,
    /// Ренеготиация аудио (микрофон) завершена — Opus пойдёт в сеть.
    AudioPublished,
    /// SFU (от лица подписчика) просит IDR — энкодер обязан выдать keyframe.
    KeyframeRequested,
    /// Оценка доступной полосы (TWCC/REMB), bps. Ретаргет энкодера.
    TargetBitrate(u64),
    /// Появился входящий трек другого участника (для роутинга декодеров).
    RemoteTrackAdded { mid: String, kind: MediaKind },
    /// Входящий Opus-пакет от участника (`mid` различает говорящих —
    /// SFU форвардит top-K=3). Приложение декодирует и микширует.
    RemoteAudio { mid: String, data: Arc<[u8]> },
    /// Участник вошёл/вышел (роутинг ростера в UI).
    ParticipantJoined {
        participant_id: Uuid,
        user_id: String,
    },
    ParticipantLeft {
        participant_id: Uuid,
        user_id: String,
    },
    /// Сервер прислал `error` — не фатально, но стоит показать.
    ServerError(String),
    /// Транспорт умер (ICE disconnected / WS закрыт / нас выкинули).
    Disconnected { reason: String },
}

/// Вид медиа во входящем треке (без утечки str0m-типов наружу).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    Audio,
    Video,
}
