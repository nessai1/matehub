//! Нативный WebRTC-паблишер для MateHub SFU (Phase 2.5).
//!
//! Крейт реализует клиентскую сторону протокола video-сервиса:
//! WS-сигналинг (`/ws/{session_id}?token=...`) + str0m в sample-mode
//! (мы — источник медиа, пакетизацию H.264 по RFC 6184 делает str0m;
//! баг репакетизации касался только релэя, см.
//! `docs/video/phase-2.5-desktop-screenshare.md` §8.2).
//!
//! Протокольный флоу (инварианты SFU, `services/video/src/sfu/mod.rs`):
//!
//! 1. `join` с оффером, содержащим только data channel — `MediaAdded` не
//!    стреляет, мёртвые треки не создаются, а `publish_track`-хинт до join
//!    сервер молча дропает (участника ещё нет в сессии).
//! 2. `publish_track {source:"screen", kind:"video"}` — строго ДО оффера.
//! 3. Клиентский `offer` с sendonly video m-line (H.264, без simulcast:
//!    v1-гейт SFU форвардит либо rid `h`, либо no-rid поток; слой `l`
//!    дропается на ингресте — слать его значит жечь аплинк впустую).
//! 4. RTCP PLI приходит как `Event::KeyframeRequest` — наверх уходит
//!    [`PublisherEvent::KeyframeRequested`], приложение обязано выдать IDR.
//!
//! Glare: если сервер прислал свой оффер, пока наш в полёте, SFU ставит наш
//! в очередь — но при дрейне устаревший оффер (серверный успел добавить
//! m-line) молча отвергается («Changed order for m-line», без Answer/Error).
//! Клиент: `accept_offer` (откатывает pending), отвечает, затем
//! `SdpApi::merge` воскрешает изменения с теми же mid, пересобирает оффер
//! под новое состояние сессии и шлёт его ПОВТОРНО. Сценарий зафиксирован
//! loopback-тестом `glare_resurrects_inflight_publish_offer`.

mod core;
mod engine;
mod error;
mod publisher;
pub mod signaling;
mod types;

pub use error::RtcClientError;
pub use publisher::{AudioSender, FrameSender, Publisher, PublisherConfig, ensure_session};
pub use types::{EncodedAudioFrame, EncodedVideoFrame, MediaKind, PublisherEvent, VideoParams};
