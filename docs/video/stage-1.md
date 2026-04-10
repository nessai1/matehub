# Video Service -- Stage 1: Working Voice/Video Call

> Цель: два человека открывают браузер, заходят в voice channel, слышат и видят друг друга.
> Один SFU-сервер, без масштабирования, без Redis, без NATS. Минимальный вертикальный срез.

---

## Scope

**Что входит:**
- WebSocket signaling server (axum)
- SFU engine на str0m (1 Rtc instance per participant)
- SDP offer/answer exchange
- ICE connectivity (ICE-lite mode)
- DTLS-SRTP media encryption
- Audio forwarding (Opus)
- Video forwarding (VP8, single layer -- без simulcast)
- Session lifecycle: create on first join, destroy on last leave
- REST endpoint для создания/получения сессии
- Базовый web-клиент на browser WebRTC API

**Что НЕ входит (Stage 2+):**
- Simulcast (multi-quality layers)
- Screen sharing
- TWCC / bandwidth estimation
- NACK retransmission
- Recording, egress
- Redis, NATS, multi-node
- JWT auth (dev mode -- токен = строка "dev-alice-token")
- TURN server
- Mobile clients

---

## Архитектура Stage 1

```
┌──────────────────────────────────────────────────────┐
│                   Video Service                       │
│                   (single process)                    │
│                                                       │
│  ┌─────────────────────┐  ┌────────────────────────┐ │
│  │  HTTP/WS Server     │  │  SFU Engine            │ │
│  │  (axum, tokio)      │  │                        │ │
│  │                     │  │  HashMap<SessionId,    │ │
│  │  POST /v1/sessions  │  │    Session {           │ │
│  │  GET  /v1/sessions  │  │      participants:     │ │
│  │  GET  /health       │  │        HashMap<        │ │
│  │                     │  │          ParticipantId, │ │
│  │  WS /ws/{session_id}│──│          Rtc           │ │
│  │    signaling        │  │        >               │ │
│  │                     │  │    }                   │ │
│  └─────────────────────┘  │  >                     │ │
│                           │                        │ │
│  ┌─────────────────────┐  │  UDP socket            │ │
│  │  Session Store      │  │  (media I/O)           │ │
│  │  (in-memory HashMap)│  │                        │ │
│  └─────────────────────┘  └────────────────────────┘ │
└──────────────────────────────────────────────────────┘
```

---

## REST API

### POST /v1/sessions

Создать или получить существующую сессию для канала.

**Request:**
```json
{
  "channel_id": "uuid"
}
```

**Response (200 / 201):**
```json
{
  "session_id": "uuid",
  "ws_url": "ws://localhost:4000/ws/{session_id}",
  "created": true
}
```

**Логика:**
- Если сессия для `channel_id` уже существует -> вернуть её (200, `created: false`)
- Если нет -> создать, сохранить в in-memory HashMap (201, `created: true`)
- Идемпотентно: повторный вызов с тем же `channel_id` -> тот же `session_id`

### GET /v1/sessions/{session_id}

**Response (200):**
```json
{
  "session_id": "uuid",
  "channel_id": "uuid",
  "participants": [
    {
      "id": "uuid",
      "user_id": "alice",
      "state": "connected"
    }
  ],
  "created_at": "2026-04-10T12:00:00Z"
}
```

### GET /health

**Response (200):** `"ok"`

---

## WebSocket Signaling Protocol

Подключение: `ws://localhost:4000/ws/{session_id}?token={dev_token}&user_id={user_id}`

Все сообщения -- JSON. Каждое имеет поле `type`.

### Client -> Server

#### join

Первое сообщение после подключения. Клиент отправляет SDP offer.

```json
{
  "type": "join",
  "sdp_offer": "v=0\r\no=- ..."
}
```

**Server отвечает:** `answer` + `participant_joined` (broadcast)

#### ice_candidate

Trickle ICE -- клиент отправляет ICE candidates по мере обнаружения.

```json
{
  "type": "ice_candidate",
  "candidate": "candidate:842163049 1 udp ...",
  "sdp_mid": "0",
  "sdp_mline_index": 0
}
```

#### leave

Явный выход из сессии.

```json
{
  "type": "leave"
}
```

### Server -> Client

#### answer

Ответ на `join` с SDP answer от SFU.

```json
{
  "type": "answer",
  "sdp_answer": "v=0\r\no=- ...",
  "participant_id": "uuid"
}
```

#### ice_candidate

SFU отправляет свои ICE candidates клиенту.

```json
{
  "type": "ice_candidate",
  "candidate": "candidate:...",
  "sdp_mid": "0",
  "sdp_mline_index": 0
}
```

#### participant_joined

Broadcast всем участникам когда кто-то присоединился.

```json
{
  "type": "participant_joined",
  "participant_id": "uuid",
  "user_id": "bob"
}
```

Получатель этого сообщения должен ожидать новые медиа-треки от SFU
(SFU добавляет remote tracks через renegotiation -- новый offer от сервера).

#### offer

SFU отправляет новый SDP offer при изменении topology
(новый участник -> новые треки для forwarding).

```json
{
  "type": "offer",
  "sdp_offer": "v=0\r\n..."
}
```

Клиент должен ответить `answer`:
```json
{
  "type": "answer",
  "sdp_answer": "v=0\r\n..."
}
```

#### participant_left

```json
{
  "type": "participant_left",
  "participant_id": "uuid",
  "user_id": "alice"
}
```

---

## Flow: два участника входят в voice channel

```
Alice (browser)          Video Service              Bob (browser)
     │                        │                          │
     │ POST /v1/sessions      │                          │
     │ {channel_id: "voice"}  │                          │
     │ ──────────────────────>│                          │
     │ <- {session_id, ws_url}│                          │
     │                        │                          │
     │ WS connect /ws/{sid}   │                          │
     │ ──────────────────────>│                          │
     │                        │                          │
     │ getUserMedia()         │                          │
     │ createOffer()          │                          │
     │                        │                          │
     │ {type: "join",         │                          │
     │  sdp_offer: "..."}     │                          │
     │ ──────────────────────>│                          │
     │                        │  Create Rtc (str0m)      │
     │                        │  ICE-lite mode           │
     │                        │  Parse offer             │
     │                        │  Generate answer         │
     │                        │                          │
     │ <- {type: "answer",    │                          │
     │     sdp_answer: "..."}  │                          │
     │                        │                          │
     │ setRemoteDescription() │                          │
     │                        │                          │
     │ <- {type: "ice_candidate"} │                      │
     │ addIceCandidate()      │                          │
     │                        │                          │
     │ ══ DTLS + SRTP ══════>│                          │
     │ ══ Audio/Video RTP ══>│                          │
     │                        │                          │
     │                        │    (Alice connected,      │
     │                        │     sending media)        │
     │                        │                          │
     │                        │  POST /v1/sessions       │
     │                        │<─────────────────────────│
     │                        │  -> same session_id      │
     │                        │─────────────────────────>│
     │                        │                          │
     │                        │  WS connect /ws/{sid}    │
     │                        │<─────────────────────────│
     │                        │                          │
     │                        │  {type: "join",          │
     │                        │   sdp_offer: "..."}      │
     │                        │<─────────────────────────│
     │                        │                          │
     │                        │  Create Rtc for Bob      │
     │                        │  Generate answer         │
     │                        │                          │
     │                        │  {type: "answer", ...}   │
     │                        │─────────────────────────>│
     │                        │                          │
     │ <- {type:              │                          │
     │  "participant_joined", │                          │
     │   user_id: "bob"}      │                          │
     │                        │                          │
     │                        │══ Bob DTLS + SRTP ══════>│
     │                        │                          │
     │  *** SFU renegotiation ***                        │
     │  SFU нужно добавить Bob's                         │
     │  треки для Alice, и Alice's                       │
     │  треки для Bob                                    │
     │                        │                          │
     │ <- {type: "offer",     │  {type: "offer",         │
     │     sdp_offer: "..."}  │   sdp_offer: "..."} ───>│
     │                        │                          │
     │ {type: "answer", ...}  │  {type: "answer", ...}  │
     │ ──────────────────────>│<─────────────────────────│
     │                        │                          │
     │ ══ Alice receives Bob's audio/video ══            │
     │ ══ Bob receives Alice's audio/video ══            │
     │                        │                          │
     │        ** CALL IN PROGRESS **                     │
     │                        │                          │
     │ {type: "leave"}        │                          │
     │ ──────────────────────>│                          │
     │                        │                          │
     │                        │ <- {type:                │
     │                        │  "participant_left",     │
     │                        │   user_id: "alice"}      │
     │                        │─────────────────────────>│
     │                        │                          │
     │                        │  SFU renegotiation:      │
     │                        │  remove Alice's tracks   │
     │                        │                          │
     │                        │  {type: "offer", ...}    │
     │                        │─────────────────────────>│
     │                        │  {type: "answer", ...}   │
     │                        │<─────────────────────────│
     │                        │                          │
     │                        │  Bob alone in session    │
     │                        │                          │
     │                        │  {type: "leave"}         │
     │                        │<─────────────────────────│
     │                        │                          │
     │                        │  Session empty ->        │
     │                        │  destroy session         │
```

---

## Модель данных (in-memory)

```rust
/// Top-level state, shared via Arc<Mutex<>>
struct AppState {
    sessions: HashMap<SessionId, Session>,
    channel_to_session: HashMap<ChannelId, SessionId>,
}

struct Session {
    id: SessionId,
    channel_id: ChannelId,
    participants: HashMap<ParticipantId, Participant>,
    created_at: Instant,
}

struct Participant {
    id: ParticipantId,
    user_id: String,
    rtc: Rtc,               // str0m WebRTC state machine
    ws_tx: mpsc::Sender<WsMessage>,  // send signaling to this participant
    state: ParticipantState,
}

enum ParticipantState {
    Connecting,  // WS connected, waiting for SDP
    Connected,   // DTLS complete, media flowing
    Disconnected,
}
```

---

## str0m Integration

### Инициализация Rtc per participant

```rust
let rtc = Rtc::builder()
    .set_ice_lite(true)           // SFU mode
    .enable_raw_packets(true)     // forward raw RTP, skip depacketization
    .build();
```

### Event loop (per participant)

Каждый participant имеет свой Rtc. Два источника событий:
1. UDP packets (media) -> `rtc.handle_input(Input::Receive(...))`
2. WebSocket messages (signaling) -> SDP/ICE processing
3. Timer ticks -> `rtc.handle_input(Input::Timeout(...))`

```rust
loop {
    // Poll str0m for outputs
    match rtc.poll_output() {
        Some(Output::Transmit(t)) => {
            udp_socket.send_to(&t.contents, t.destination).await?;
        }
        Some(Output::Event(Event::MediaData(data))) => {
            // Forward to all other participants in the session
            for other in session.participants.values() {
                if other.id != self.id {
                    other.rtc.writer(mid).write_rtp(...)?;
                }
            }
        }
        Some(Output::Event(Event::IceConnectionStateChange(state))) => {
            // Update participant state
        }
        Some(Output::Timeout(deadline)) => {
            // Schedule wakeup
        }
        None => break,
    }
}
```

### UDP Socket Strategy (Stage 1)

**Один UDP socket** для всех participants. str0m в ICE-lite mode использует
один local candidate (IP:port). Демультиплексация по source address:

```
UDP socket :4001
  ├── packet from 1.2.3.4:50000 -> Alice's Rtc
  ├── packet from 5.6.7.8:60000 -> Bob's Rtc
  └── packet from 9.10.11.12:70000 -> Charlie's Rtc
```

Маппинг `remote_addr -> ParticipantId` строится при ICE connectivity checks.

---

## Порты

| Порт | Назначение |
|------|-----------|
| 4000 | HTTP REST + WebSocket signaling |
| 4001 | UDP media (SFU <-> browsers) |

---

## Web Client (Stage 1)

Минимальный HTML/JS client для тестирования. Не React -- plain browser API.
Размещается на фронтенде как `/hub/channel/[channelId]` page, но основная
логика -- vanilla JS с browser WebRTC API.

### Client flow

```javascript
// 1. Create session (or get existing)
const { session_id, ws_url } = await fetch('/v1/sessions', {
  method: 'POST',
  body: JSON.stringify({ channel_id })
}).then(r => r.json());

// 2. Get user media
const stream = await navigator.mediaDevices.getUserMedia({
  audio: true,
  video: { width: 640, height: 480 }
});

// 3. Create peer connection
const pc = new RTCPeerConnection({
  iceServers: [] // no TURN in Stage 1
});

// Add local tracks
stream.getTracks().forEach(track => pc.addTrack(track, stream));

// 4. Connect WebSocket
const ws = new WebSocket(`${ws_url}?token=dev-alice-token&user_id=alice`);

// 5. Create and send offer
const offer = await pc.createOffer();
await pc.setLocalDescription(offer);
ws.send(JSON.stringify({ type: 'join', sdp_offer: offer.sdp }));

// 6. Handle server answer
ws.onmessage = async (e) => {
  const msg = JSON.parse(e.data);
  
  if (msg.type === 'answer') {
    await pc.setRemoteDescription({ type: 'answer', sdp: msg.sdp_answer });
  }
  
  if (msg.type === 'ice_candidate') {
    await pc.addIceCandidate({
      candidate: msg.candidate,
      sdpMid: msg.sdp_mid,
      sdpMLineIndex: msg.sdp_mline_index,
    });
  }
  
  if (msg.type === 'offer') {
    // SFU renegotiation (new participant joined)
    await pc.setRemoteDescription({ type: 'offer', sdp: msg.sdp_offer });
    const answer = await pc.createAnswer();
    await pc.setLocalDescription(answer);
    ws.send(JSON.stringify({ type: 'answer', sdp_answer: answer.sdp }));
  }
};

// 7. Send ICE candidates
pc.onicecandidate = (e) => {
  if (e.candidate) {
    ws.send(JSON.stringify({
      type: 'ice_candidate',
      candidate: e.candidate.candidate,
      sdp_mid: e.candidate.sdpMid,
      sdp_mline_index: e.candidate.sdpMLineIndex,
    }));
  }
};

// 8. Handle remote tracks
pc.ontrack = (e) => {
  // Attach remote stream to <video> element
  const remoteVideo = document.getElementById('remote-video');
  remoteVideo.srcObject = e.streams[0];
};
```

---

## Definition of Done

Stage 1 считается завершённым когда:

1. [ ] `cargo run -p matehub-video` запускается, слушает :4000 (HTTP/WS) и :4001 (UDP)
2. [ ] `POST /v1/sessions` создаёт сессию, повторный вызов возвращает существующую
3. [ ] WebSocket подключение устанавливается с `session_id`
4. [ ] SDP offer от клиента принимается, SDP answer возвращается
5. [ ] ICE connectivity устанавливается (ICE-lite)
6. [ ] DTLS handshake завершается успешно
7. [ ] Аудио от Alice пересылается Bob'у и наоборот
8. [ ] Видео от Alice пересылается Bob'у и наоборот
9. [ ] `participant_joined` / `participant_left` broadcast работает
10. [ ] SFU renegotiation при join/leave второго участника
11. [ ] Сессия уничтожается когда последний участник вышел
12. [ ] Работает в Chrome (минимум), Firefox (желательно)

---

## Файловая структура (результат)

```
services/video/
├── Cargo.toml
└── src/
    ├── main.rs              # Entry point: start HTTP + UDP servers
    ├── config.rs            # Ports, dev mode flag
    ├── state.rs             # AppState, Session, Participant structs
    ├── api/
    │   ├── mod.rs           # axum Router
    │   ├── sessions.rs      # POST/GET /v1/sessions
    │   └── ws.rs            # WebSocket handler (/ws/{session_id})
    ├── sfu/
    │   ├── mod.rs           # SFU engine entry point
    │   ├── rtc_loop.rs      # str0m event loop per participant
    │   ├── media_router.rs  # Forward media between participants
    │   └── udp.rs           # UDP socket, demux by remote addr
    └── signaling/
        ├── mod.rs
        └── messages.rs      # JSON message types (serde)
```

---

## Известные ограничения Stage 1

1. **Нет simulcast** -- видео всегда в одном качестве. Если bandwidth плохой -- фризы.
2. **Нет NACK/RTX** -- потерянные пакеты не ретранслируются. При packet loss > 2% -- артефакты.
3. **Нет TURN** -- за symmetric NAT / corporate firewall не пробьётся.
4. **Single thread media** -- все Rtc на одном tokio runtime. Для 2-3 участников хватит.
5. **In-memory state** -- restart сервиса = все сессии потеряны.
6. **Нет auth** -- любой с dev-токеном может подключиться.
7. **Renegotiation complexity** -- при 3+ участниках SFU должен renegotiate с каждым. Это O(N) SDP exchanges при каждом join/leave. Работает для малых групп, не масштабируется.
