# MateHub Video Service -- Техническая спецификация

> Rust-based SFU с минимальным потреблением ресурсов и гибкой масштабируемостью.
> Автономный сервис: можно вырвать и воткнуть куда угодно.

---

## 1. Обзор ландшафта и обоснование решений

### 1.1 Существующие open-source решения

| Проект | Язык | Тип | Масштабирование | Лицензия | Вердикт |
|--------|------|-----|-----------------|----------|---------|
| **LiveKit** | Go (Pion) | SFU | Каскадные SFU, Redis | Apache 2.0 | Эталон архитектуры, но Go |
| **Jitsi Meet** | Java/Kotlin | SFU | Multi-JVB каскадирование | Apache 2.0 | Зрелый, но тяжёлый JVM-стек |
| **MediaSoup** | C++/Node.js | SFU-библиотека | Pipe transports (ручное) | ISC | Отличная библиотека, не сервер |
| **Janus** | C | Plugin gateway | Только single-node | GPL v3 | GPL -- poison pill для коммерции |
| **Pion/WebRTC** | Go | Библиотека | N/A | MIT | Building block для Go SFU |
| **str0m** | Rust | Библиотека | N/A | MIT/Apache 2.0 | Лучший Rust WebRTC стек |
| **webrtc-rs** | Rust | Библиотека | N/A | MIT/Apache 2.0 | Порт Pion, менее активен |
| **atm0s-media** | Rust | SFU (experimental) | Mesh SFU-нод | N/A | Концепт, не production |

**Ключевой вывод:** Production-ready open-source SFU на Rust не существует. Это одновременно и вызов, и возможность -- мы строим то, чего ещё нет на рынке.

### 1.2 Как устроен Zoom (эталон индустрии)

Zoom использует **MMR (Multimedia Router)** -- проприетарный SFU:

- **Без транскодирования** в hot path -- чистый packet forwarding
- **Каскадные SFU** по регионам через приватный backbone (не публичный интернет)
- **Simulcast** как основная стратегия адаптивного качества (не SVC)
- **Kernel-bypass networking** (вероятно DPDK) для минимальной латентности
- **E2EE через MLS-протокол** -- но с отключением серверных фич (запись, микширование)
- **Аудио микшируется server-side** (top 3-4 активных спикера) -- дёшево по CPU
- **Anycast routing** к edge-нодам + приватный backbone между дата-центрами
- **Проприетарный протокол** (не WebRTC), с WebRTC-gateway для браузеров

**Что берём от Zoom:** архитектуру каскадных SFU, simulcast-first подход, разделение signaling/media, принцип "SFU не декодирует медиа".

### 1.3 Почему Rust

| Аспект | Rust | Go (LiveKit) | C++ (MediaSoup) |
|--------|------|-------------|-----------------|
| GC-паузы | Нет | ~0.5-1ms (p999 хуже) | Нет |
| Память на соединение | ~30-50 KB | ~50-100 KB | ~30-50 KB |
| Zero-copy forwarding | Нативно | Fragile (escape analysis) | Нативно |
| Memory safety | Гарантирована | Гарантирована | Нет |
| p99/p999 латентность | Предсказуемая | Fat tail от GC | Предсказуемая |
| io_uring | Да (tokio-uring, glommio) | Нет | Да |
| Соединений на сервер | 2-3x больше чем Go | Базовый уровень | 2-3x больше чем Go |

Rust даёт memory safety как у Go + производительность как у C++. Для real-time media это оптимальная комбинация.

---

## 2. Архитектура системы

### 2.1 Общая архитектура MateHub

```
                        ┌──────────────┐
                        │  API Gateway │
                        │  (HTTP/WS)   │
                        └──────┬───────┘
                               │
        ┌──────────┬───────────┼───────────┬──────────┐
        │          │           │           │          │
   ┌────▼────┐ ┌───▼───┐ ┌────▼────┐ ┌────▼────┐ ┌───▼─────┐
   │  Auth   │ │ Chat  │ │ Video   │ │ General │ │Presence │
   │ Service │ │Service│ │Signaling│ │ Service │ │ Service │
   └─────────┘ └───────┘ └────┬────┘ └─────────┘ └─────────┘
                               │
                          ┌────▼────┐
                          │   SFU   │
                          │  Nodes  │
                          └────┬────┘
                               │
                    ┌──────────┼──────────┐
                    │          │          │
               ┌────▼──┐ ┌────▼──┐ ┌────▼────┐
               │ TURN  │ │Egress │ │Recording│
               │Server │ │(RTMP) │ │ Service │
               └───────┘ └───────┘ └─────────┘
```

### 2.2 Архитектура Video Service (автономный модуль)

```
┌─────────────────────────────────────────────────────────┐
│                    VIDEO SERVICE                         │
│                                                         │
│  ┌─────────────────────┐   ┌──────────────────────┐    │
│  │   Signaling Server  │   │   Management API     │    │
│  │   (WebSocket/HTTP)  │   │   (HTTP/gRPC)        │    │
│  │   - SDP exchange    │   │   - Room CRUD        │    │
│  │   - ICE candidates  │   │   - Participant mgmt │    │
│  │   - Room events     │   │   - Stats/monitoring │    │
│  └──────────┬──────────┘   └──────────┬───────────┘    │
│             │                         │                 │
│             ▼                         ▼                 │
│  ┌──────────────────────────────────────────────┐      │
│  │           Coordination Layer                  │      │
│  │   (Redis: room state, node registry,          │      │
│  │    pub/sub, load metrics)                     │      │
│  └──────────────────┬───────────────────────────┘      │
│                     │                                   │
│  ┌──────────────────▼───────────────────────────┐      │
│  │              SFU Engine (Rust core)            │      │
│  │                                               │      │
│  │  ┌─────────┐  ┌──────────┐  ┌────────────┐  │      │
│  │  │  ICE    │  │  DTLS    │  │   SRTP     │  │      │
│  │  │ Agent   │  │ Handler  │  │ Encrypt/   │  │      │
│  │  │(str0m)  │  │(rustls)  │  │ Decrypt    │  │      │
│  │  └────┬────┘  └────┬─────┘  └─────┬──────┘  │      │
│  │       │            │              │          │      │
│  │  ┌────▼────────────▼──────────────▼──────┐   │      │
│  │  │         Media Router                   │   │      │
│  │  │  - Simulcast layer selection           │   │      │
│  │  │  - RTCP feedback handling              │   │      │
│  │  │  - Bandwidth estimation (TWCC/GCC)     │   │      │
│  │  │  - Jitter buffer                       │   │      │
│  │  │  - Audio level detection               │   │      │
│  │  │  - Packet fan-out (zero-copy)          │   │      │
│  │  └───────────────────────────────────────┘   │      │
│  └──────────────────────────────────────────────┘      │
│                                                         │
│  ┌──────────────────────────────────────────────┐      │
│  │           Event Bus (NATS/Redis Streams)      │      │
│  │  - room.created, room.ended                   │      │
│  │  - participant.joined, participant.left        │      │
│  │  - track.published, track.muted               │      │
│  │  - active_speaker.changed                     │      │
│  │  - recording.started, recording.stopped       │      │
│  └──────────────────────────────────────────────┘      │
└─────────────────────────────────────────────────────────┘
```

### 2.3 Двухплоскостная архитектура SFU-ноды

Критически важное разделение -- **control plane** и **media plane** на разных потоках:

```
┌─────────────────────────────────────────────────┐
│                  SFU Node Process                │
│                                                  │
│  Control Plane (tokio runtime)                   │
│  ┌────────────────────────────────────────────┐  │
│  │ - WebSocket signaling handlers             │  │
│  │ - Room/session state machine               │  │
│  │ - REST/gRPC management API                 │  │
│  │ - Redis pub/sub (coordination)             │  │
│  │ - Health reporting                         │  │
│  └──────────────┬─────────────────────────────┘  │
│                 │ lock-free channels              │
│                 │ (crossbeam / ring buffer)       │
│  Media Plane (dedicated OS threads)              │
│  ┌──────────────▼─────────────────────────────┐  │
│  │ - UDP socket I/O (recvmmsg/sendmmsg)       │  │
│  │ - STUN/DTLS/SRTP demux                     │  │
│  │ - str0m protocol state machine             │  │
│  │ - RTP parsing + forwarding (zero-copy)     │  │
│  │ - SRTP encrypt/decrypt (ring crate)        │  │
│  │ - Jitter buffer management                 │  │
│  │ - TWCC/GCC bandwidth estimation            │  │
│  │ - Simulcast layer switching                │  │
│  │ - io_uring (Linux) / kqueue (macOS)        │  │
│  └────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────┘
```

---

## 3. Стек технологий

### 3.1 Core

| Компонент | Технология | Обоснование |
|-----------|------------|-------------|
| WebRTC стек | **str0m** | Sans-I/O дизайн, Rust-native, максимальный контроль над I/O |
| Async runtime | **tokio** | Control plane: signaling, API, координация |
| Media I/O | Dedicated threads + **io_uring** (Linux) | Предсказуемая латентность, batched syscalls |
| Crypto (SRTP) | **ring** | BoringSSL assembly, AES-GCM hardware acceleration |
| TLS/DTLS | **rustls** | Интегрируется с str0m |
| Signaling transport | **axum** (HTTP/WS) | Совместим с tokio, production-ready |
| Serialization | **protobuf** (prost) | Signaling messages, inter-node communication |

### 3.2 Infrastructure

| Компонент | Технология | Обоснование |
|-----------|------------|-------------|
| Координация | **Redis** | Room state, node registry, pub/sub, load metrics |
| Event bus | **NATS** | Кросс-сервисные события (video -> chat, presence) |
| Inter-SFU | **gRPC** (tonic) | Каскадное соединение между SFU нодами |
| TURN | **coturn** | Стандарт индустрии, deploy per region |
| Container | **Docker** | Деплой, изоляция |
| Orchestration | **Kubernetes** | Горизонтальное масштабирование SFU нод |

### 3.3 Codecs (FFI bindings, используются только для recording/egress)

| Кодек | Crate/Library | Назначение |
|-------|---------------|------------|
| VP8/VP9 | libvpx (vpx-sys) | Базовый WebRTC видеокодек |
| H.264 | OpenH264 (openh264-sys) | Максимальная HW совместимость |
| AV1 | rav1e (encode), dav1d (decode) | Next-gen, royalty-free |
| Opus | opus-rs / audiopus | Аудио (mandatory в WebRTC) |
| Media pipelines | gstreamer-rs | Recording, transcoding, RTMP egress |

**Важно:** SFU не декодирует и не кодирует медиа в основном пути. Кодеки нужны только для recording/egress сервисов.

---

## 4. Детальное ТЗ по компонентам

### 4.1 SFU Engine (ядро)

#### 4.1.1 WebRTC Transport Layer

**Задачи:**
- [ ] ICE Agent: full ICE с поддержкой trickle ICE (RFC 8838), ICE restart
- [ ] STUN client/server: Binding requests/responses для connectivity checks
- [ ] DTLS handshake: key exchange через rustls, fingerprint verification из SDP
- [ ] SRTP key derivation: DTLS-SRTP exporter для получения медиа-ключей
- [ ] SRTP encrypt/decrypt: AES-128-CM + HMAC-SHA1-80 (in-place, zero-copy)
- [ ] SCTP over DTLS: Data channels для signaling metadata
- [ ] BUNDLE: мультиплексирование всех медиа через один transport
- [ ] rtcp-mux: RTP и RTCP на одном порту

**Реализация:** Всё это обеспечивает str0m. Наша задача -- интеграция с I/O layer.

#### 4.1.2 Media Router

**Задачи:**
- [ ] RTP packet parsing: zero-copy разбор заголовков (SSRC, seq, timestamp, PT, extensions)
- [ ] Packet fan-out: один входящий пакет -> N исходящих (через Arc<[u8]> из slab pool)
- [ ] SSRC/sequence rewriting: для каждого subscriber свои sequence numbers (непрерывные)
- [ ] Simulcast layer selection: выбор слоя per subscriber на основе bandwidth + UI layout
- [ ] Active speaker detection: анализ audio level (RTP header extension) для определения спикера
- [ ] Track subscription management: какие треки получает каждый subscriber

**Производительность:**
- Slab allocator для RTP-пакетов (пул буферов MTU=1500, zero allocation на hot path)
- Ring buffer для jitter compensation (фиксированный размер ~512 entries, reuse in-place)
- recvmmsg/sendmmsg для batch I/O (5-10x снижение syscall overhead)
- io_uring на Linux для batched async I/O

#### 4.1.3 Congestion Control & Quality Adaptation

**Задачи:**
- [ ] TWCC (Transport-Wide Congestion Control): сбор arrival timestamps, feedback packets
- [ ] GCC (Google Congestion Control): delay-based + loss-based bandwidth estimation
- [ ] REMB: fallback receiver-side bandwidth estimation
- [ ] Adaptive simulcast switching: автоматический выбор слоя при изменении bandwidth
- [ ] Pacer: равномерная отправка пакетов (anti-burst)
- [ ] FEC (Forward Error Correction): FlexFEC для proactive loss recovery
- [ ] NACK/RTX: retransmission по запросу (для loss rate < 5%)
- [ ] PLI/FIR: keyframe requests при потере reference frames

#### 4.1.4 Jitter Buffer

**Задачи:**
- [ ] Adaptive jitter buffer для видео: динамический размер на основе jitter statistics
- [ ] Audio jitter buffer: NetEQ-подобный алгоритм (20-200ms adaptive)
- [ ] Packet reordering: восстановление порядка по sequence numbers
- [ ] Gap detection: определение потерь для NACK

### 4.2 Signaling Server

#### 4.2.1 WebSocket Signaling

**Задачи:**
- [ ] Persistent WebSocket connection per participant
- [ ] SDP offer/answer exchange (JSON + SDP)
- [ ] ICE candidate trickle (incremental candidate exchange)
- [ ] Room lifecycle events (join, leave, mute, unmute, screen share)
- [ ] Participant roster sync
- [ ] Reconnection handling с session resumption

**Протокол сообщений (Protobuf):**
```
enum SignalMessageType {
    JOIN_ROOM = 0;
    LEAVE_ROOM = 1;
    OFFER = 2;
    ANSWER = 3;
    ICE_CANDIDATE = 4;
    TRACK_PUBLISHED = 5;
    TRACK_UNPUBLISHED = 6;
    TRACK_MUTED = 7;
    TRACK_UNMUTED = 8;
    SPEAKER_CHANGED = 9;
    PARTICIPANT_JOINED = 10;
    PARTICIPANT_LEFT = 11;
    SUBSCRIPTION_UPDATE = 12;
    QUALITY_UPDATE = 13;
    ROOM_UPDATE = 14;
}
```

#### 4.2.2 Session Management

Video Service оперирует **SFU Sessions**, а не комнатами. Комната (channel) -- сущность General Service. Video Service знает только `channel_id` как внешний ключ.

**Задачи:**
- [ ] Session lifecycle: create on first join, destroy on last leave
- [ ] Session-to-channel binding: session привязана к channel_id (1:0..1)
- [ ] Participant management: join, leave, kick, permissions (из JWT)
- [ ] Track management: publish, unpublish, mute, unmute
- [ ] Subscription management: кто получает какие треки и в каком качестве
- [ ] Session limits: max participants (из channel config), max duration (для DM calls)
- [ ] Session state persistence в Redis (для multi-node, Phase 2)
- [ ] Session events -> Event Bus (NATS)
- [ ] Duplicate session handling: если user заходит с двух вкладок -- два participant'а с одним user_id, оба получают медиа
- [ ] Disconnection timeout: participant считается offline через 15 секунд без ICE connectivity, state держится ещё 30 секунд для reconnect

**Session state machine:**
```
         create (first join)
              │
              ▼
         ┌─────────┐
         │  ACTIVE  │ <── participants > 0
         └────┬─────┘
              │ last participant left
              ▼
         ┌─────────┐
         │ DRAINING │ ── 30 sec grace period (reconnect window)
         └────┬─────┘
              │ timeout / no rejoin
              ▼
         ┌─────────┐
         │ CLOSED  │ ── cleanup, NATS event, Redis cleanup
         └─────────┘
```

#### 4.2.3 SDP Negotiation Engine

**Задачи:**
- [ ] SDP generation: создание server-side SDP offers
- [ ] SDP parsing: разбор client offers
- [ ] Codec negotiation: выбор поддерживаемых кодеков (VP8, VP9, H.264, AV1, Opus)
- [ ] Simulcast negotiation: rid, simulcast SDP attributes
- [ ] BUNDLE и rtcp-mux negotiation
- [ ] Header extension negotiation (TWCC, abs-send-time, audio-level)

### 4.3 Management API

**Задачи:**
- [ ] REST API (HTTP/JSON) для внешней интеграции
- [ ] gRPC API для внутренних сервисов

**Endpoints:**

```
# Sessions (привязаны к channel_id из General Service)
POST   /v1/sessions                          -- создать/получить сессию для channel
       body: {channel_id, type: "voice"|"stage"|"dm_call", config?}
       response: {session_id, ws_url, ice_servers}
       Идемпотентно: если сессия для channel уже есть -- вернуть существующую.

GET    /v1/sessions?channel_id={cid}         -- найти активную сессию по channel
GET    /v1/sessions/{sid}                    -- информация о сессии
DELETE /v1/sessions/{sid}                    -- принудительно закрыть сессию

# Participants (в контексте сессии)
GET    /v1/sessions/{sid}/participants       -- список участников
POST   /v1/sessions/{sid}/participants/{pid}/remove  -- удалить участника
POST   /v1/sessions/{sid}/participants/{pid}/mute    -- server-side mute

# Recording (в контексте сессии)
POST   /v1/sessions/{sid}/recording/start    -- начать запись
POST   /v1/sessions/{sid}/recording/stop     -- остановить запись
GET    /v1/recordings/{rid}                  -- метаданные записи
GET    /v1/recordings/{rid}/url              -- presigned URL для скачивания

# Auth
POST   /v1/token                             -- генерация participant token
       body: {user_id, channel_id, permissions: {canPublish, canSubscribe, ...}}
       Вызывается General Service при join.

# Ops
GET    /v1/health                            -- liveness (процесс жив)
GET    /v1/ready                             -- readiness (Redis, NATS подключены)
GET    /v1/stats                             -- метрики для dashboard
GET    /v1/nodes                             -- список SFU-нод и их нагрузка
```

**Аутентификация (два уровня):**
- **Service-to-service:** Management API (`/v1/sessions`, `/v1/token`) защищён API key или mTLS. Вызывается General Service, не клиентом напрямую.
- **Client-to-SFU:** WebSocket signaling endpoint защищён JWT participant token (сгенерирован через `POST /v1/token`). Video Service валидирует подпись (shared secret или public key от Auth Service).

### 4.4 Координация и масштабирование

#### 4.4.1 Node Registry

**Задачи:**
- [ ] Регистрация SFU-ноды при запуске (IP, port, region, capacity)
- [ ] Heartbeat каждые 3 секунды с метриками нагрузки
- [ ] Автоматическое удаление при пропуске N heartbeats
- [ ] Хранение в Redis (hash per node, TTL-based expiry)

**Метрики ноды:**
```rust
struct NodeMetrics {
    node_id: String,
    region: String,
    cpu_usage: f32,          // 0.0 - 1.0
    memory_usage: f32,
    bandwidth_in: u64,       // bytes/sec
    bandwidth_out: u64,
    active_rooms: u32,
    active_participants: u32,
    active_streams: u32,
}
```

#### 4.4.2 Room Assignment

**Задачи:**
- [ ] Geographic-aware allocation: первый участник -> SFU в его регионе
- [ ] Load-aware scheduling: назначение на наименее загруженную ноду в регионе
- [ ] Room migration: перемещение комнаты при деградации ноды
- [ ] Cascading trigger: создание каскадного SFU при участниках из другого региона

**Алгоритм назначения:**
1. Определить регион участника (по IP / explicit region hint)
2. Найти ноды в этом регионе с load < 70%
3. Если комната уже существует на ноде в этом регионе -- направить туда
4. Если комната на ноде в другом регионе -- cascade или redirect
5. Если нет нод в регионе -- fallback на ближайший регион

#### 4.4.3 Cascading SFU

**Задачи:**
- [ ] Inter-SFU gRPC connection: установка каскадного канала между нодами
- [ ] Media forwarding: один экземпляр каждого стрима per SFU-to-SFU link
- [ ] Simulcast layer negotiation: remote SFU запрашивает нужный слой
- [ ] Keyframe propagation: PLI/FIR через каскадный канал
- [ ] Participant state sync: roster, mute status, speaker across SFUs

**Когда каскадировать:**
- Участники в 2+ регионах, по 2+ в каждом
- Латентность от участника до primary SFU > 150ms
- НЕ каскадировать для 1:1 звонков или если remote participant один

### 4.5 TURN Server

**Задачи:**
- [ ] Деплой coturn per region (минимум: EU, US, Asia)
- [ ] TURN over UDP (primary)
- [ ] TURN over TCP (fallback)
- [ ] TURN over TLS на порту 443 (корпоративные firewall'ы)
- [ ] Geographic DNS routing к ближайшему TURN серверу
- [ ] Credential generation (time-limited TURN credentials через HMAC)
- [ ] Мониторинг bandwidth usage (TURN дорогой -- ~$0.09/GB на AWS)

**Оценка:** ~10-20% соединений потребуют TURN. В enterprise -- до 30-40%.

### 4.6 Recording Service

#### Архитектура

Recording Service -- **отдельный процесс**, подключается к комнате как скрытый participant.
НЕ записывать на SFU-нодах -- они оптимизированы для forwarding.

```
                   ┌───────────┐
                   │    SFU    │
                   └─────┬─────┘
                         │ subscribes to all tracks
                         │ (as hidden participant)
                   ┌─────▼─────┐
                   │ Recording │
                   │  Worker   │
                   └─────┬─────┘
                    ┌────┴────┐
                    ▼         ▼
            ┌──────────┐ ┌──────────────┐
            │Individual│ │  Composite   │
            │  Tracks  │ │ (GStreamer/  │
            │(RTP dump)│ │  headless    │
            │          │ │  browser)    │
            └────┬─────┘ └──────┬───────┘
                 │              │
                 ▼              ▼
            ┌──────────────────────┐
            │  Object Storage      │
            │  (S3 / MinIO)        │
            └──────────┬───────────┘
                       │
                       ▼
            ┌──────────────────────┐
            │  Post-processing     │
            │  (ffmpeg / GStreamer) │
            │  RTP -> WebM/MP4     │
            └──────────────────────┘
```

#### Два режима записи

**Individual track recording (Phase 3 baseline):**
- [ ] Recording Worker подписывается на все треки комнаты
- [ ] Сохраняет RTP payloads per participant (audio + video + screen отдельно)
- [ ] Метаданные: timestamps, SSRC mapping, participant ID, track type
- [ ] Результат: N файлов, синхронизированных по timestamps
- [ ] Post-processing: remux через ffmpeg/GStreamer -> WebM/MP4
- [ ] CPU cost: минимальный (no decode, just write to disk)
- [ ] Применение: архив, compliance, post-production с custom layout

**Composite recording (Phase 3 extended):**
- [ ] Headless browser (Chromium) или GStreamer pipeline присоединяется к комнате
- [ ] Декодирует все треки, композитит в единый layout (grid / speaker view)
- [ ] Энкодит результат в H.264+AAC -> MP4
- [ ] Результат: один готовый к просмотру файл
- [ ] CPU cost: высокий (~1 core per 720p composite, больше при >10 участниках)
- [ ] Применение: sharing, playback, embed

#### Lifecycle записи

```
1. POST /v1/rooms/{id}/recording/start {mode: "individual"|"composite", layout?}
2. Recording Worker создаётся, подключается к комнате
3. NATS event: recording.started {room_id, recording_id, mode}
4. Chunks загружаются в S3 (multipart upload, каждые 5-10 секунд)
5. POST /v1/rooms/{id}/recording/stop   (или room.ended -> auto-stop)
6. Post-processing (если individual -> опциональный composite)
7. NATS event: recording.ready {room_id, recording_id, url, duration, size}
8. URL доступен через GET /v1/recordings/{id}
```

#### Взаимодействие recording с screen sharing

Когда участник делает screen share во время записи:
- Individual mode: screen track записывается как отдельный файл
- Composite mode: screen share автоматически занимает основную область layout,
  камера спикера переходит в маленькое окно (picture-in-picture)

#### Storage и retention

- [ ] Chunked upload в S3 (multipart, не ждать окончания -- crash safety)
- [ ] Metadata в БД: room_id, start/end, duration, participants, storage URL, mode, size
- [ ] Lifecycle policy: hot storage (S3 Standard) -> cold (S3 Glacier) через N дней
- [ ] CDN для playback (HLS/DASH adaptive streaming)
- [ ] Quota per workspace/channel (configurable)

### 4.7 Egress Service (Streaming)

**Задачи:**
- [ ] RTMP output для стриминга на YouTube/Twitch
- [ ] Compositor: decode всех треков -> layout -> encode H.264+AAC -> RTMP
- [ ] Реализация через GStreamer pipeline (gstreamer-rs)
- [ ] Configurable layouts (speaker, grid, custom)
- [ ] HLS/DASH output для low-latency просмотра

### 4.8 Event Bus Integration

**Задачи:**
- [ ] Публикация событий в NATS при всех значимых действиях
- [ ] Формат событий: protobuf
- [ ] Webhook delivery для внешних интеграций
- [ ] Подписка Chat Service на события (системные сообщения в канале)
- [ ] Подписка Presence Service на события (статус "в звонке")

**NATS subject hierarchy:**
```
matehub.video.session.{session_id}.started
matehub.video.session.{session_id}.ended
matehub.video.session.{session_id}.participant.joined
matehub.video.session.{session_id}.participant.left
matehub.video.session.{session_id}.track.published
matehub.video.session.{session_id}.track.muted
matehub.video.session.{session_id}.speaker.changed
matehub.video.session.{session_id}.recording.started
matehub.video.session.{session_id}.recording.ready

# Wildcard subscriptions:
matehub.video.session.*.participant.*     -- Chat Service: системные сообщения
matehub.video.session.*.started           -- Presence Service: статус "в звонке"
matehub.video.session.*.recording.ready   -- Storage Service: post-processing
```

**Payload событий (protobuf):**
```
session.started      {session_id, channel_id, type, node_id, timestamp}
session.ended        {session_id, channel_id, duration_sec, peak_participants, timestamp}
participant.joined   {session_id, channel_id, user_id, region, timestamp}
participant.left     {session_id, channel_id, user_id, reason: "leave"|"kick"|"timeout", timestamp}
track.published      {session_id, user_id, track_id, kind: "audio"|"video"|"screen", timestamp}
track.muted          {session_id, user_id, track_id, kind, muted: bool, timestamp}
speaker.changed      {session_id, active_speakers: [{user_id, audio_level}], timestamp}
recording.started    {session_id, recording_id, mode: "individual"|"composite", timestamp}
recording.ready      {session_id, recording_id, url, duration_sec, size_bytes, timestamp}
call.ringing         {session_id, channel_id, caller_id, callee_ids, timestamp}  -- DM calls only
call.missed          {session_id, channel_id, caller_id, callee_ids, timestamp}  -- DM calls only
```

Все события содержат `channel_id` чтобы потребители (Chat, Presence) могли маппить на свои сущности без обращения к Video Service.

---

## 5. Протоколы и форматы

### 5.1 WebRTC Protocol Stack

```
Application (Video Service logic)
         │
    Signaling (WebSocket + Protobuf)
         │
    SDP (RFC 8866) -- offer/answer model
         │
    ┌────┴────┐
    │         │
  SRTP      SCTP over DTLS
(media)    (data channels)
    │         │
    └────┬────┘
         │
    DTLS 1.2 (RFC 6347) -- key exchange
         │
    ICE (RFC 8445) -- NAT traversal
         │
    STUN (RFC 8489) / TURN (RFC 8656)
         │
    UDP (primary) / TCP (fallback)
```

### 5.2 Поддерживаемые кодеки

**Видео (приоритет сверху вниз):**
1. **VP8** -- mandatory-to-implement (RFC 7742), baseline совместимость
2. **H.264** -- mandatory-to-implement, максимальная HW поддержка
3. **VP9** -- 30-40% лучше сжатие чем VP8, SVC support
4. **AV1** -- next-gen, 30-50% лучше VP9 (когда HW encode станет повсеместным)

**Аудио:**
1. **Opus** -- mandatory (RFC 6716), 20-32 kbps для voice, in-band FEC

### 5.3 Simulcast Configuration

**Camera video (3 слоя):**
```
High:  1080p / 720p  @ 30fps  ~2.5 Mbps
Mid:   480p / 360p   @ 30fps  ~500 kbps
Low:   180p / 120p   @ 15fps  ~150 kbps

Общий upload клиента: ~3-3.5 Mbps
```

**Screen share (2 слоя):**
```
High:  Native res (1080p-4K) @ 5-15fps  ~1-2.5 Mbps  (detail-optimized, text readable)
Low:   480p / 360p           @ 5fps     ~200 kbps     (thumbnail/preview)

Общий upload: ~1.5-3 Mbps
```

Screen share отличается от камеры:
- Ниже framerate (текст и UI не требуют 30fps, но требуют pixel-perfect резкость)
- Выше resolution (native экрана, чтобы текст был читаем)
- Другой encoding mode: `contentHint: "detail"` для приоритета резкости над плавностью
- 2 simulcast слоя вместо 3 (экран либо смотрят целиком, либо в превью)
- Может идти параллельно с камерой (участник показывает экран + его лицо видно)

SFU выбирает слой per subscriber на основе:
- Доступной bandwidth (TWCC/GCC estimation)
- Размера отображения участника в UI (fullscreen vs thumbnail)
- Явного запроса подписчика (pin high quality)
- Типа трека: screen share автоматически получает высокий приоритет при fullscreen

---

## 6. Интеграция со смежными сервисами

### 6.1 Контракт Video Service

Video Service -- автономный модуль. Для интеграции с MateHub (или любой другой системой) нужны:

**Входящие зависимости (что Video Service потребляет):**
- **Auth Service** -- JWT токены для аутентификации участников
- **Redis** -- координация, room state
- **NATS** -- event bus
- **Object Storage** (S3/MinIO) -- для записей

**Исходящие контракты (что Video Service предоставляет):**
- REST/gRPC API для управления комнатами
- WebSocket endpoint для клиентского подключения
- Event stream через NATS (события комнат/участников)
- Webhook callbacks

### 6.2 Интеграция с Chat Service

Chat привязан к channel. Voice/video session -- это медиа-слой того же channel. Chat Service слушает NATS события и вставляет системные сообщения в историю канала:

```
Chat Service                          Video Service
     │                                      │
     │  (Chat Service НЕ вызывает Video     │
     │   Service напрямую. Он подписан      │
     │   на NATS events.)                   │
     │                                      │
     │  NATS: matehub.video.session.*.participant.joined
     │ <──────────────────────────────────── │
     │  -> системное сообщение в чате       │
     │     канала: "User A joined voice"    │
     │                                      │
     │  NATS: matehub.video.session.*.ended │
     │ <──────────────────────────────────── │
     │  -> "Voice session ended (15:32)"    │
     │                                      │
     │  NATS: matehub.video.session.*.recording.ready
     │ <──────────────────────────────────── │
     │  -> "Recording available" + ссылка   │
```

### 6.3 Интеграция с General Service (профили, каналы)

General Service владеет Channel. При join пользователя в voice channel, General Service:
1. Проверяет permissions пользователя для этого канала
2. Запрашивает participant token у Video Service
3. Отдаёт token + connection info клиенту

```
Client          General Service              Video Service
  │                   │                           │
  │  "Join voice"     │                           │
  │ ─────────────────>│                           │
  │                   │  POST /v1/sessions        │
  │                   │  {channel_id, type}       │
  │                   │ ─────────────────────────>│
  │                   │  <- {session_id, ws_url}  │
  │                   │                           │
  │                   │  POST /v1/token           │
  │                   │  {user_id, channel_id,    │
  │                   │   permissions}            │
  │                   │ ─────────────────────────>│
  │                   │  <- {jwt_token}           │
  │                   │                           │
  │  <- {ws_url,      │                           │
  │      token,       │                           │
  │      ice_servers} │                           │
  │                   │                           │
  │  WebSocket connect (с JWT token)              │
  │ ─────────────────────────────────────────────>│
  │  SDP offer/answer, ICE, media flow            │
  │ <────────────────────────────────────────────>│
```

### 6.4 Модель каналов (Discord-style) -- ОСНОВНАЯ МОДЕЛЬ

#### Три уровня абстракции

```
┌─────────────────────────────────────────────────────────────────┐
│  Channel (General Service)                                      │
│  - Persistent entity, живёт от создания до удаления             │
│  - Хранит: name, type, permissions, chat history                │
│  - Принадлежит workspace (серверу)                              │
│  - Типы: voice, stage, DM-call                                  │
│  - НЕ знает о медиа, SFU, WebRTC                               │
└──────────────────────────────┬──────────────────────────────────┘
                               │ channel_id (1:0..1)
┌──────────────────────────────▼──────────────────────────────────┐
│  SFU Session (Video Service)                                    │
│  - Transient: создаётся когда первый участник заходит           │
│  - Уничтожается когда последний участник вышел                  │
│  - Привязана к channel_id                                       │
│  - Содержит: media routing state, SRTP contexts, bandwidth est. │
│  - Может НЕ существовать, если в канале никого нет              │
└──────────────────────────────┬──────────────────────────────────┘
                               │ session_id -> node assignment
┌──────────────────────────────▼──────────────────────────────────┐
│  SFU Node (Infrastructure)                                      │
│  - Kubernetes Pod, обслуживает десятки-сотни сессий              │
│  - Масштабируется HPA по метрикам нагрузки                      │
│  - Одна нода = пул для множества каналов                        │
└─────────────────────────────────────────────────────────────────┘
```

#### Типы каналов

| Тип канала | Поведение | SFU Session lifecycle |
|------------|-----------|----------------------|
| **Voice Channel** | Persistent. Список участников виден всем в workspace. Нет "звонка" -- просто join/leave. Chat привязан к каналу. | Создаётся при первом join, уничтожается при последнем leave. Channel продолжает жить. |
| **Stage Channel** | Persistent. Роли: speaker (canPublish) / listener (subscribe only). Модерация: "поднять руку", approve/reject. | Аналогично voice. Speakers отдельно от listeners в permissions. |
| **DM Call** | Ephemeral. Один user звонит другому (или группе). Рингтон, accept/reject. Channel создаётся ad-hoc, удаляется после звонка. | Создаётся при инициации, уничтожается при завершении. Channel тоже удаляется (или переходит в "ended" state). |

#### Lifecycle voice/stage channel

```
[Channel создан в General Service]
         │
         ▼
   Channel exists, SFU Session = null
   (канал пустой, никто не разговаривает,
    но виден в списке, chat доступен)
         │
         │  User A нажимает "Join"
         ▼
   Video Service: POST /v1/sessions {channel_id}
   -> SFU Session создаётся на свободной ноде
   -> User A подключается по WebRTC
   -> NATS event: session.started {channel_id}
   -> Presence: "User A в голосовом канале"
         │
         │  User B нажимает "Join"
         ▼
   Video Service: user B присоединяется к существующей SFU Session
   -> NATS event: participant.joined {channel_id, user_id}
   -> Presence обновлён
         │
         │  User A выходит
         ▼
   SFU Session всё ещё жива (User B остался)
         │
         │  User B выходит (последний)
         ▼
   SFU Session уничтожается (cleanup media state)
   -> NATS event: session.ended {channel_id, duration}
   Channel продолжает существовать.
   Chat history сохранён.
```

#### Lifecycle DM call

```
[User A звонит User B]
         │
         ▼
   General Service создаёт ephemeral channel
   Video Service: POST /v1/sessions {channel_id, type: "dm_call"}
   -> SFU Session создаётся
   -> NATS event: call.ringing {channel_id, caller, callee}
   -> Push notification / in-app ring для User B
         │
    ┌────┴────┐
    ▼         ▼
  Accept    Reject/Timeout (30s)
    │         │
    ▼         ▼
  User B    SFU Session уничтожается
  joins     Channel удаляется
    │       NATS: call.missed {channel_id}
    ▼
  Нормальный звонок (как voice channel)
    │
    │  Оба вышли
    ▼
  SFU Session уничтожается
  Channel переходит в "ended" (chat history сохранён)
```

#### Pod-модель в Kubernetes

```
НЕ ДЕЛАТЬ: один pod = один канал
  (1000 каналов = 1000 подов, 950 пустых, k8s scheduler плачет)

ДЕЛАТЬ: pool SFU-нод, каждая обслуживает множество каналов

┌──────────────────────────────────────────┐
│  SFU Node Pool (Kubernetes Deployment)   │
│                                          │
│  sfu-node-0: sessions [ch-1, ch-5, ch-9]│
│  sfu-node-1: sessions [ch-2, ch-3]      │
│  sfu-node-2: sessions [ch-7, ch-11, ...]│
│  sfu-node-3: (idle, готов к нагрузке)    │
│                                          │
│  HPA scales on:                          │
│  - aggregate CPU > 60%                   │
│  - aggregate bandwidth > 70% capacity    │
│  - active_sessions / node > threshold    │
└──────────────────────────────────────────┘
```

Масштабирование:
- **Scale up:** HPA добавляет ноды когда средняя нагрузка превышает порог
- **Scale down:** Нода помечается как draining, новые сессии не назначаются, существующие доживают, после опустошения -- pod удаляется
- **Pod-per-channel оправдан ТОЛЬКО** для enterprise multi-tenancy с жёсткой изоляцией (отдельный вопрос)

---

## 7. Безопасность

### 7.1 Encryption

- [ ] **Hop-by-hop**: DTLS-SRTP (стандартный WebRTC) -- всегда включён
- [ ] **E2EE (опционально)**: SFrame (RFC 9605) через Insertable Streams API
  - Key agreement через MLS (RFC 9420)
  - SFU видит только RTP headers, payload зашифрован
  - Отключает: server-side recording, audio mixing, transcription
  - Dependency Descriptor в RTP header для SVC layer switching без доступа к payload
- [ ] **Signaling**: WSS (TLS 1.3)
- [ ] **Management API**: HTTPS + JWT auth
- [ ] **Inter-SFU**: mTLS

### 7.2 Authentication & Authorization

- [ ] JWT-based auth для всех подключений
- [ ] Per-room permissions в токене: canPublish, canSubscribe, canRecord, isAdmin
- [ ] Token expiry + refresh mechanism
- [ ] Rate limiting на signaling endpoint

### 7.3 DDoS Mitigation

- [ ] STUN cookie exchange для защиты от amplification
- [ ] Rate limiting на ICE candidates
- [ ] Max streams per participant
- [ ] Room capacity limits

---

## 8. Мониторинг и observability

### 8.1 Метрики (Prometheus)

**SFU-level:**
- `sfu_active_rooms` -- количество активных комнат
- `sfu_active_participants` -- количество участников
- `sfu_active_streams` -- количество медиа-стримов
- `sfu_packets_received_total` / `sfu_packets_sent_total`
- `sfu_bytes_received_total` / `sfu_bytes_sent_total`
- `sfu_cpu_usage` / `sfu_memory_usage`
- `sfu_packet_forward_latency_us` -- гистограмма латентности forwarding (p50, p99, p999)

**Per-participant:**
- `participant_rtt_ms` -- round-trip time
- `participant_jitter_ms` -- jitter
- `participant_packet_loss_ratio` -- packet loss
- `participant_nack_count` -- количество retransmission requests
- `participant_pli_count` -- количество keyframe requests
- `participant_bandwidth_estimate_bps` -- estimated bandwidth

### 8.2 Logging

- Structured logging (tracing crate + tracing-subscriber)
- Per-room trace ID для корреляции
- Уровни: ERROR (packet loss > 25%, ICE failed), WARN (high NACK rate, layer downgrade), INFO (join/leave, room lifecycle), DEBUG (individual packet events)

---

## 9. План реализации (фазы)

### Phase 1: MVP -- 1:1 и малые группы (до 10 участников)

**Цель:** Работающие видеозвонки 1:1 и малые группы на одном SFU-сервере.

1. **SFU Engine на str0m**
   - WebRTC transport (ICE, DTLS, SRTP)
   - Базовый media router (packet forwarding, SSRC rewrite)
   - Simulcast support (3 layers)
   - NACK/PLI handling
   - TWCC bandwidth estimation

2. **Signaling Server**
   - WebSocket endpoint (axum)
   - SDP offer/answer
   - ICE candidate exchange
   - Room join/leave

3. **Room Management**
   - In-memory room state (single node)
   - Basic participant management
   - Track publish/subscribe

4. **Screen Sharing**
   - Отдельный track type `source: screen` (через `getDisplayMedia()` на клиенте)
   - SFU форвардит как обычный video track -- специальной логики не требуется
   - 2 simulcast слоя (high: native res 5-15fps, low: thumbnail 5fps)
   - Может идти параллельно с camera track (лицо + экран одновременно)
   - Signaling: `track.published {kind: screen}` -> UI других участников показывает экран

5. **Базовый клиент**
   - Web-клиент на JavaScript (browser WebRTC API)
   - Join room, publish camera/mic, subscribe to others
   - Simulcast sending
   - Adaptive quality (layer switching)
   - Screen share (getDisplayMedia + publish as screen track)

6. **TURN**
   - Деплой coturn (single instance)
   - Credential generation

### Phase 2: Масштабирование и production-readiness

**Цель:** Горизонтальное масштабирование, большие комнаты (до 100 участников).

1. **Redis координация**
   - Room state в Redis
   - Node registry + heartbeat
   - Load-aware room assignment

2. **Multi-node SFU**
   - Room partitioning across nodes
   - Каскадные SFU (inter-node media forwarding через gRPC)

3. **Management API**
   - REST API для room CRUD
   - JWT token generation
   - Stats/monitoring endpoints

4. **Event Bus**
   - NATS интеграция
   - Публикация room/participant events
   - Webhook delivery

5. **Observability**
   - Prometheus метрики
   - Structured logging
   - Grafana dashboards

### Phase 3: Enterprise features

**Цель:** Recording, streaming, E2EE, large-scale.

1. **Recording Service**
   - Individual track recording
   - Composite recording (GStreamer)
   - S3 storage + metadata

2. **Egress Service**
   - RTMP streaming to YouTube/Twitch
   - HLS output

3. **E2EE**
   - SFrame implementation
   - MLS key agreement
   - Dependency Descriptor support

4. **Large rooms (100+ participants)**
   - Optimized fan-out
   - Server-side audio mixing (top-N speakers)
   - Gallery view pagination (отправка только видимых стримов)

5. **Screen sharing (advanced)**
   - Annotation overlay (рисование поверх расшаренного экрана)
   - Remote control (опционально, через data channel)
   - Selective window/tab share (browser API)

### Phase 4: Geo-distribution

**Цель:** Multi-region deployment с минимальной латентностью для глобальных пользователей.

1. **Multi-region TURN** (coturn per region + geo DNS)
2. **Regional SFU clusters** (Kubernetes per region)
3. **Cascading SFU optimization** (backbone routing, smart cascade decisions)
4. **Edge routing** (anycast или latency-based DNS для signaling)

---

## 10. Нефункциональные требования

| Параметр | Требование |
|----------|-----------|
| Латентность (glass-to-glass, same region) | < 150ms |
| Латентность (cross-region, cascaded) | < 300ms |
| Packet forwarding latency (SFU) | < 1ms p99 |
| Участников на SFU-ноду | 500+ (при 720p simulcast) |
| Комнат на SFU-ноду | 100+ |
| CPU overhead per stream (forwarding) | < 0.1% core |
| Память per connection | < 50 KB |
| Время подключения (ICE + DTLS) | < 2 секунды (p95) |
| Reconnection time | < 3 секунды |
| Uptime | 99.9% |
| Packet loss tolerance | Нормальная работа при < 5%, деградация при < 15% |
| TURN fallback | 100% success rate через TURN/TLS:443 |

---

## 11. Зависимости (Cargo.toml core crates)

```toml
# WebRTC protocol stack
str0m = "*"                    # Sans-I/O WebRTC

# Async runtime & networking
tokio = { features = ["full"] }
axum = "*"                     # HTTP + WebSocket signaling
tonic = "*"                    # gRPC (inter-SFU, management)
prost = "*"                    # Protobuf serialization

# Crypto
ring = "*"                     # AES-GCM, HMAC (SRTP)
rustls = "*"                   # TLS/DTLS

# Data & coordination
redis = { features = ["tokio-comp"] }
async-nats = "*"               # NATS client

# Concurrency
crossbeam = "*"                # Lock-free channels
parking_lot = "*"              # Faster mutexes (control plane)

# Observability
tracing = "*"
tracing-subscriber = "*"
prometheus = "*"

# Serialization
serde = { features = ["derive"] }
serde_json = "*"

# Auth
jsonwebtoken = "*"             # JWT validation

# Utils
uuid = "*"
rand = "*"
bytes = "*"
```

---

## 12. Открытые вопросы и решения, требующие принятия

| # | Вопрос | Варианты | Рекомендация |
|---|--------|----------|-------------|
| 1 | Client SDK: писать свой или использовать browser API? | Свой SDK (TypeScript) / Чистый browser WebRTC | Phase 1: browser API + thin wrapper. Phase 2: полноценный SDK |
| 2 | Mobile clients | WebView + browser API / Native SDK (Swift, Kotlin) | Phase 1: WebView. Phase 3: Native SDKs |
| 3 | Audio mixing server-side или client-side? | Server-side (Opus decode/mix/encode) / Client-side (N streams) | До 10 участников -- client-side. 10+ -- server-side top-N mixing |
| 4 | Recording: composite vs individual tracks? | Composite (GStreamer) / Individual (RTP dump) | Оба: individual для архива, composite on-demand |
| 5 | Event bus: NATS vs Redis Streams? | NATS / Redis Streams | NATS (масштабируется лучше, Redis уже нагружен координацией) |
| 6 | SFU-to-SFU: gRPC vs plain RTP? | gRPC (control + media) / RTP (media) + gRPC (control) | RTP для медиа + gRPC для control plane (меньше overhead на медиа) |
| 7 | Лицензия | MIT / Apache 2.0 / Dual | Apache 2.0 (как LiveKit) |

---

## Приложение A: Глоссарий

| Термин | Определение |
|--------|------------|
| **SFU** | Selective Forwarding Unit -- пересылает медиа без транскодирования |
| **MCU** | Multipoint Control Unit -- декодирует, микширует, кодирует (дорого по CPU) |
| **ICE** | Interactive Connectivity Establishment -- NAT traversal framework |
| **STUN** | Session Traversal Utilities for NAT -- discovery публичного IP |
| **TURN** | Traversal Using Relays around NAT -- relay когда прямое соединение невозможно |
| **DTLS** | Datagram TLS -- TLS для UDP |
| **SRTP** | Secure RTP -- шифрование медиа-пакетов |
| **SDP** | Session Description Protocol -- описание медиа-сессии |
| **TWCC** | Transport-Wide Congestion Control -- per-packet feedback для bandwidth estimation |
| **GCC** | Google Congestion Control -- delay-based + loss-based алгоритм |
| **NACK** | Negative ACK -- запрос ретрансмиссии потерянного пакета |
| **PLI** | Picture Loss Indication -- запрос keyframe |
| **FIR** | Full Intra Request -- принудительный keyframe |
| **FEC** | Forward Error Correction -- избыточность для recovery без retransmission |
| **Simulcast** | Отправка нескольких quality layers одного источника |
| **SVC** | Scalable Video Coding -- один bitstream с встроенными слоями качества |
| **E2EE** | End-to-End Encryption -- шифрование, которое SFU не может расшифровать |
| **SFrame** | Secure Frame -- стандарт framing для E2EE медиа (RFC 9605) |
| **MLS** | Messaging Layer Security -- протокол группового key agreement (RFC 9420) |

## Приложение B: Ключевые RFC

| RFC | Тема |
|-----|------|
| RFC 8825 | WebRTC Overview |
| RFC 8829 | JSEP |
| RFC 8866 | SDP |
| RFC 8445 | ICE |
| RFC 8838 | Trickle ICE |
| RFC 8489 | STUN |
| RFC 8656 | TURN |
| RFC 6347 | DTLS 1.2 |
| RFC 5764 | DTLS-SRTP |
| RFC 3711 | SRTP |
| RFC 3550 | RTP |
| RFC 4585 | RTCP Feedback |
| RFC 7741 | VP8 RTP Payload |
| RFC 6184 | H.264 RTP Payload |
| RFC 6716 | Opus Codec |
| RFC 8627 | FlexFEC |
| RFC 9605 | SFrame |
| RFC 9420 | MLS |
| RFC 9143 | BUNDLE |
