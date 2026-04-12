# Video Service -- Stage 2: Production-Quality Calls

> Цель: надёжные видеозвонки для 10 участников без growing delay,
> с server-side renegotiation, simulcast, screen sharing, и TURN.
> Подготовка к масштабированию (Redis, NATS, multi-node).

---

## Prerequisite: Stage 1 Bug Fixes

Stage 2 не начинается пока не закрыты критические баги Stage 1.

### P-1. Fix growing delay (BUG-1 CRITICAL)

**Проблема:** `writer.write()` без промежуточного `poll_output()` копит пакеты в очереди str0m.

**Fix:** Переписать `poll_all_outputs()` на interleaved architecture:

```
for each participant:
    loop poll_output():
        Transmit -> send UDP
        MediaData -> IMMEDIATELY write to each recipient:
            writer.write(...)
            drain_transmits(recipient) // poll until Timeout
        Timeout -> break
```

**Решение borrow checker:** Использовать indexed access вместо iterator:
```rust
let pids: Vec<ParticipantId> = session.participants.keys().copied().collect();
for i in 0..pids.len() {
    let pid = pids[i];
    // poll participant pid
    // on MediaData: iterate other participants by index
}
```

**Definition of Done:** Двусторонний звонок 5+ минут без роста delay.

### P-2. Server-side renegotiation

**Проблема:** Сейчас initial SDP offer от клиента, с tracks захардкоженными при connect. Клиент не может динамически публиковать/убирать треки.

**Fix:** Перевести на LiveKit-style flow:

```
Client                          SFU
  |  connect (empty offer)       |
  |  recvonly transceivers       |
  | ---------------------------> |
  |  answer                      |
  | <--------------------------- |
  |                              |
  |  signal: publish_track       |
  |  {kind: "audio"}             |
  | ---------------------------> |
  |                              |
  |  SFU creates offer           |
  |  with new sendrecv m-line    |
  | <--------------------------- |
  |  answer (with track attached)|
  | ---------------------------> |
  |  media flows                 |
```

**Signaling messages (новые):**
```json
// Client -> Server
{"type": "publish_track", "kind": "audio"|"video"|"screen"}
{"type": "unpublish_track", "track_id": "..."}
{"type": "mute_track", "track_id": "...", "muted": true}

// Server -> Client
{"type": "track_published", "track_id": "...", "mid": "..."}
{"type": "track_muted", "participant_id": "...", "track_id": "...", "muted": true}
```

**Definition of Done:** Клиент подключается без медиа, включает mic/cam через кнопки, SFU renegotiate.

### P-3. Stream ID mapping fix (BUG-2)

**Проблема:** `stream.id` от str0m не матчится с `participantId` во фронте.

**Fix:** При server-side renegotiation, SFU контролирует `stream_id` в SDP offer. Использовать `participant_id` как stream_id. Фронт маппит `ontrack stream.id` -> participant напрямую.

**Definition of Done:** Оба браузера видят видео друг друга без heuristic matching.

---

## Scope Stage 2

### 2.1 Simulcast

Клиент отправляет 3 quality layers одного видео:

```
Camera -> Encoder:
  High:  720p  @ 30fps  ~1.5 Mbps
  Mid:   360p  @ 20fps  ~500 kbps
  Low:   180p  @ 15fps  ~150 kbps
```

SFU выбирает layer per subscriber на основе:
- Bandwidth estimation (TWCC/GCC)
- Размер отображения в UI (fullscreen vs thumbnail)
- Явный запрос от клиента (`subscription_update` message)

**SDP:** Клиент добавляет `a=simulcast:send h;m;l` + `a=rid` атрибуты.

**SFU:** str0m отдаёт `Event::MediaData` с `rid` полем. SFU фильтрует по rid при forwarding.

**SDK:** Конфигурация simulcast encodings при `addTransceiver`:
```javascript
pc.addTransceiver(track, {
  direction: "sendonly",
  sendEncodings: [
    { rid: "h", maxBitrate: 1500000, scaleResolutionDownBy: 1 },
    { rid: "m", maxBitrate: 500000, scaleResolutionDownBy: 2 },
    { rid: "l", maxBitrate: 150000, scaleResolutionDownBy: 4 },
  ]
});
```

### 2.2 Screen sharing

- Отдельный track type `source: screen` через `getDisplayMedia()`
- 2 simulcast слоя (high: native res 5-15fps, low: thumbnail)
- `contentHint: "detail"` для приоритета резкости текста
- Может идти параллельно с camera track
- SFU forwarding без специальной логики -- просто ещё один video track

### 2.3 NACK/RTX retransmission

str0m поддерживает NACK из коробки. Нужно:
- Настроить RTX cache: 100-150 пакетов per stream (вместо дефолта ~500-1000)
- Убедиться что `Event::KeyframeRequest` propagate к source participant
- PLI/FIR при потере keyframe

**Экономия памяти:** 500 conn * 4 streams: дефолт ~2.4 GB -> тюнинг ~240 MB.

### 2.4 TWCC bandwidth estimation

str0m генерирует TWCC feedback и имеет BWE модуль (`bwe/trendline_estimator.rs`).

Задачи:
- Получать estimated bandwidth per subscriber
- Использовать для выбора simulcast layer: `if bw < 600kbps -> forward "l" layer`
- Пробросить bandwidth hints через signaling (`quality_update` message)

### 2.5 Mute/unmute через signaling

```
Client clicks mute -> track.enabled = false
  -> signaling: {"type": "mute_track", "track_id": "...", "muted": true}
  -> SFU broadcasts to all: {"type": "track_muted", ...}
  -> Other clients show muted icon
```

SFU может продолжать получать comfort noise (если клиент DTX включён) или пустые фреймы. Не forwarding'ить muted audio -- экономия bandwidth.

### 2.6 Dedicated media thread

Вынести SFU engine из tokio на `std::thread`:

```
Before:
  tokio::spawn(engine.run())  // async, на tokio runtime

After:
  std::thread::spawn(|| engine.run_blocking())  // dedicated OS thread
  // engine.run_blocking() использует std::net::UdpSocket
  // с set_read_timeout, без async
```

Коммуникация: `crossbeam::channel` между tokio (signaling) и media thread.

**Зачем:** Предсказуемая латентность. tokio runtime может задержать media task на время poll других задач. Dedicated thread -- гарантированный scheduling.

### 2.7 TURN server

Деплой coturn для пользователей за NAT/corporate firewall.

- TURN over UDP (primary)
- TURN over TCP (fallback)
- TURN over TLS на порту 443 (корпоративные firewalls)
- Time-limited credentials через HMAC
- ~10-20% соединений потребуют TURN

**SDK:** Передавать ICE servers при connect:
```javascript
iceServers: [
  { urls: "stun:stun.matehub.io:3478" },
  { urls: "turn:turn.matehub.io:3478", username: "...", credential: "..." },
  { urls: "turns:turn.matehub.io:443", username: "...", credential: "..." },
]
```

**Backend:** Endpoint для генерации TURN credentials: `GET /v1/turn-credentials`

### 2.8 JWT auth

Заменить `dev-*-token` на реальные JWT:

```json
{
  "sub": "user_id",
  "hub_id": "...",
  "channel_id": "...",
  "permissions": {
    "canPublish": true,
    "canSubscribe": true
  },
  "exp": 1712345678
}
```

- Video Service валидирует подпись (shared secret с General Service)
- Token expiry + refresh через signaling
- Permissions проверяются при `publish_track`

### 2.9 Redis integration

**Node registry:**
```
HSET sfu:nodes:{node_id} ip "192.168.1.10" port 4001 region "eu" cpu 0.35 ...
EXPIRE sfu:nodes:{node_id} 10  // TTL = 3 heartbeats
```

**Room assignment:**
```
HSET sfu:sessions:{session_id} node_id "..." channel_id "..." created_at "..."
SET sfu:channel_to_session:{channel_id} {session_id}
```

**Heartbeat:** Каждые 3 секунды, EXPIRE refreshes TTL.

### 2.10 NATS events

**Subject hierarchy:**
```
matehub.video.session.{session_id}.started
matehub.video.session.{session_id}.ended
matehub.video.session.{session_id}.participant.joined
matehub.video.session.{session_id}.participant.left
matehub.video.session.{session_id}.track.published
matehub.video.session.{session_id}.track.muted
matehub.video.session.{session_id}.speaker.changed
```

Chat Service подписывается на `matehub.video.session.*.participant.*` для системных сообщений.

### 2.11 Observability

**Prometheus метрики:**
- `sfu_active_sessions` / `sfu_active_participants` / `sfu_active_streams`
- `sfu_packets_forwarded_total` / `sfu_bytes_forwarded_total`
- `sfu_packet_forward_latency_us` (histogram: p50, p99, p999)
- `participant_rtt_ms` / `participant_jitter_ms` / `participant_packet_loss_ratio`

**Structured logging:**
- tracing crate с per-session trace ID
- JSON format для Loki/ELK

**Health endpoints:**
- `GET /v1/health` -- liveness
- `GET /v1/ready` -- readiness (Redis, NATS connected)
- `GET /v1/stats` -- текущие метрики

---

## SDK Changes (packages/sdk-video)

### Новый connect flow

```typescript
// 1. Connect без медиа
const client = new VideoClient({ serverUrl, sessionId, userId, token });
client.on(handler);
await client.connect();  // WS + PeerConnection (recvonly)

// 2. Пользователь нажимает "enable mic"
await client.enableMic();
// -> getUserMedia({audio})
// -> signaling: publish_track {kind: "audio"}
// -> server sends offer with new m-line
// -> client answers, attaches track
// -> media flows

// 3. Пользователь нажимает "enable camera"
await client.enableCamera();
// -> аналогично, с simulcast encodings

// 4. Mute
client.muteMic();  // track.enabled = false + signaling
client.unmuteMic();
```

### Новые events

```typescript
type VideoClientEvent =
  | { type: "connected" }
  | { type: "participant_joined"; participant: Participant }
  | { type: "participant_left"; participantId: string }
  | { type: "track_published"; participantId: string; track: MediaStreamTrack }
  | { type: "track_muted"; participantId: string; trackKind: string; muted: boolean }
  | { type: "speaking_changed"; participantId: string; speaking: boolean }
  | { type: "quality_changed"; participantId: string; quality: "high"|"mid"|"low" }
  | { type: "disconnected"; reason: string }
  | { type: "error"; message: string };
```

---

## Frontend Changes

### Voice channel UI

- Кнопки mic/cam OFF по дефолту, включаются при клике
- Индикатор muted на плитке участника (перечёркнутый микрофон)
- Quality indicator на плитке (зелёный/жёлтый/красный по bandwidth)
- Screen share: при включении -- занимает основную область, камера в PiP
- Speaking indicator: зелёный border при `speaking_changed` event

### Dev toolbar

- Работает: переключение пользователей через URL params
- Добавить: кнопка "Simulate poor network" (throttle bandwidth для тестирования simulcast)

---

## Нефункциональные требования Stage 2

| Параметр | Требование |
|----------|-----------|
| Участников в комнате | до 10 (без degradation) |
| Латентность (same network) | < 200ms glass-to-glass |
| Packet forwarding latency | < 1ms p99 |
| Видео quality adaptation | < 2 секунды на смену layer |
| Reconnection time | < 3 секунды |
| TURN fallback success | 100% через TLS:443 |
| Время до первого фрейма | < 3 секунды после join |

---

## Архитектурные закладки для Stage 3

Stage 2 должен заложить абстракции, чтобы Stage 3 был **добавлением модулей**,
а не переписыванием. Каждая закладка -- конкретный trait или интерфейс.

### A-1. Transport trait (для каскадных SFU)

Media router сейчас работает напрямую с `Rtc` (str0m). В Stage 3 каскадная
SFU нода выглядит как ещё один "participant", но с gRPC/RTP транспортом
вместо WebRTC.

**Закладка:** Абстрагировать транспорт за trait:

```rust
trait MediaSink {
    fn write_media(&mut self, mid: Mid, pt: Pt, time: MediaTime, data: &[u8]) -> Result<()>;
    fn poll_output(&mut self) -> Result<Output>;
}

// Stage 2: единственная реализация
struct WebRtcSink { rtc: Rtc }

// Stage 3: добавляется
struct CascadeSink { grpc_stream: ... }
```

Media router работает с `dyn MediaSink`, не с `Rtc` напрямую.
При добавлении cascade в Stage 3 -- новая реализация trait'а, router не меняется.

**Объём:** ~50 строк trait + ~100 строк refactor forwarding loop.

### A-2. Session store trait (для multi-node)

Сейчас session state в `HashMap<SessionId, Session>` (in-memory).
Stage 2 добавляет Redis. Stage 3 добавляет координацию между нодами.

**Закладка:** Абстрагировать session store:

```rust
#[async_trait]
trait SessionStore {
    async fn get_session(&self, id: SessionId) -> Option<SessionInfo>;
    async fn assign_node(&self, channel_id: ChannelId) -> NodeAssignment;
    async fn register_node(&self, metrics: NodeMetrics);
}

// Stage 2
struct RedisSessionStore { pool: RedisPool }

// Stage 3: тот же RedisSessionStore, но с логикой cascade routing
```

**Объём:** ~30 строк trait. Redis implementation и так пишется в Stage 2.

### A-3. Participant abstraction (для recording и cascade)

Сейчас `SfuParticipant` жёстко привязан к `Rtc` + `ws_tx`.
В Stage 3 participant может быть:
- Browser (WebRTC Rtc + WebSocket)
- Каскадная SFU нода (gRPC + RTP)
- Recording bot (подписан на медиа, ничего не отправляет)

**Закладка:** Participant как composition, не monolith:

```rust
struct SfuParticipant {
    id: ParticipantId,
    user_id: String,
    transport: Box<dyn MediaSink>,  // WebRTC или Cascade
    signaling: Box<dyn SignalingSink>,  // WS или gRPC
    tracks_in: Vec<TrackIn>,
    tracks_out: Vec<TrackOut>,
}

trait SignalingSink: Send {
    fn send(&self, msg: ServerMessage) -> Result<()>;
}

// Stage 2: WebSocket sink
struct WsSink { tx: mpsc::UnboundedSender<ServerMessage> }

// Stage 3: gRPC sink для cascade
struct GrpcSink { stream: ... }

// Stage 3: Null sink для recording bot
struct NullSink;
```

**Объём:** ~40 строк traits + ~80 строк refactor participant struct.

### A-4. SRTP key extraction probe

Stage 3 hybrid pipeline требует извлечения SRTP keying material из str0m
после DTLS handshake. **Это открытый вопрос** -- неизвестно, даёт ли str0m API доступ.

**Закладка в Stage 2:** Написать integration test который:
1. Устанавливает DTLS через str0m
2. Пытается извлечь SRTP master key + salt
3. Если удаётся -- документирует API
4. Если нет -- документирует что нужен upstream PR или fork

```rust
#[test]
fn can_extract_srtp_keys() {
    // ... setup Rtc, complete DTLS ...
    // Try to access keying material
    // Document result in str0m-analysis.md
}
```

**Объём:** ~50 строк тест. Результат определяет feasibility Stage 3 hybrid pipeline.

### A-5. Event bus abstraction (для recording triggers)

Stage 2 добавляет NATS. Stage 3 recording service подписывается на события.

**Закладка:** Events уже определены в Stage 2 NATS schema. Убедиться что
`recording.started` / `recording.ready` events включены в schema даже если
recording ещё не реализован. Это позволит recording service в Stage 3
подключиться без изменений в video service.

**Объём:** 0 дополнительного кода -- просто не удалять эти events из NATS schema.

### A-6. Dependency Descriptor header extension

Stage 3 E2EE требует Dependency Descriptor (DD) в RTP header для SVC
layer selection без доступа к encrypted payload.

**Закладка в Stage 2:** При настройке simulcast (2.1), включить DD header
extension в SDP negotiation. str0m имеет частичную поддержку DD.
Даже если в Stage 2 DD не используется для layer selection, его наличие
в SDP означает что браузер будет отправлять DD, и в Stage 3 SFU сможет
читать его для E2EE-compatible forwarding.

**Объём:** ~10 строк в SDP generation.

---

### Итого: overhead закладок

| Закладка | Объём | Что даёт для Stage 3 |
|----------|-------|---------------------|
| A-1 Transport trait | ~150 строк | Cascade SFU без refactor router |
| A-2 Session store trait | ~30 строк | Multi-node без refactor state |
| A-3 Participant abstraction | ~120 строк | Recording + Cascade без refactor |
| A-4 SRTP key probe | ~50 строк | Feasibility check для hybrid pipeline |
| A-5 Event schema | 0 строк | Recording service plug-and-play |
| A-6 DD header | ~10 строк | E2EE-ready simulcast |
| **Total** | **~360 строк** | **Stage 3 = добавление модулей** |

360 строк дополнительного кода в Stage 2 экономят ~4000 строк рефакторинга
в Stage 3. ROI: 11x.

---

## Порядок реализации

### Фаза A: Фиксы, фундамент и архитектурные закладки (1-2 недели)

1. **P-1: Fix growing delay** -- interleaved poll/write
2. **A-1: Transport trait** -- абстракция MediaSink (закладка для cascade)
3. **A-3: Participant abstraction** -- composition вместо monolith
4. **P-2: Server-side renegotiation** -- publish_track signaling
5. **P-3: Stream ID mapping** -- автоматически решается при P-2
6. **2.6: Dedicated media thread** -- std::thread вместо tokio task
7. **jemalloc** как global allocator
8. **A-4: SRTP key extraction probe** -- integration test

### Фаза B: Quality of media (1-2 недели)

9. **2.1: Simulcast** -- 3 layers, SFU layer selection
10. **A-6: Dependency Descriptor** -- DD header extension в SDP (E2EE-ready)
11. **2.3: NACK/RTX** -- RTX cache tuning (100-150 пакетов)
12. **2.4: TWCC bandwidth estimation** -- adaptive quality
13. **2.5: Mute/unmute** -- signaling + frontend indicators

### Фаза C: Features (1 неделя)

14. **2.2: Screen sharing** -- getDisplayMedia + separate track
15. **2.8: JWT auth** -- заменить dev tokens
16. **2.7: TURN server** -- coturn deploy + credential generation

### Фаза D: Infrastructure (1 неделя)

17. **A-2: Session store trait** -- абстракция (закладка для multi-node)
18. **2.9: Redis** -- session state, node registry (реализация trait'а)
19. **2.10: NATS** -- event bus для Chat/Presence (A-5: включить recording events в schema)
20. **2.11: Observability** -- Prometheus + structured logging

---

## Definition of Done (Stage 2)

### Функциональность
1. [ ] Двусторонний звонок 10 минут БЕЗ growing delay
2. [ ] Оба участника видят и слышат друг друга
3. [ ] Mic/Camera OFF по дефолту, включаются через кнопки (server-side renegotiation)
4. [ ] Simulcast: SFU переключает layers при throttle bandwidth
5. [ ] Screen sharing работает
6. [ ] Mute отображается на плитках других участников
7. [ ] TURN: подключение за NAT работает
8. [ ] JWT auth: dev tokens заменены
9. [ ] Redis: session state persists при restart signaling
10. [ ] NATS: Chat Service получает participant.joined/left
11. [ ] Prometheus: базовые метрики доступны на /metrics
12. [ ] 10 участников в одной комнате без degradation

### Архитектурные закладки для Stage 3
13. [ ] MediaSink trait: media router работает через абстракцию, не напрямую с Rtc
14. [ ] Participant -- composition (transport + signaling за traits)
15. [ ] SessionStore trait: Redis реализация за абстракцией
16. [ ] SRTP key extraction probe: integration test + документация результата
17. [ ] Dependency Descriptor header в SDP negotiation
18. [ ] NATS schema включает recording.started/ready events

### Тесты
19. [ ] 36+ (Stage 1) + новые для simulcast, auth, mute, renegotiation
20. [ ] Integration test: 3 участника, один включает/выключает камеру, другие видят

---

## Известные ограничения (Stage 3+)

- Нет multi-node (каскадные SFU)
- Нет recording/egress
- Нет E2EE
- Single SFU node (вертикальное масштабирование)
- Нет mobile SDK
- Нет gallery view pagination (все стримы всегда forwarded)
