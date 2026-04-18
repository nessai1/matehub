# MateHub -- Roadmap

Phased plan, derived from `docs/chat_research.md` and `docs/video-service-spec.md`.
Status tracked per-item: **done** / **in-progress** / **planned**.

---

## Video Service (SFU)

### Stage 1 -- Basic SFU (done)
- [x] str0m + tokio, UDP per-participant
- [x] SDP offer/answer, ICE candidates via WS signaling
- [x] Media forwarding: audio + video between 2+ participants
- [x] Trickle ICE (no gathering-complete wait)
- [x] Sequential WS message queue in SDK (race fix)

### Stage 2 -- Stability & scale (done)
- [x] Interleaved poll/forward (no batched forwarding delay)
- [x] PLI keyframe request on TrackOut Open
- [x] KeyframeRequest routing to publisher (not broadcaster)
- [x] Mute signaling via WS (not RTP)
- [x] ICE zombie watchdog (disconnected + no UDP > 30s)
- [x] Speaking detection (voice frequency bins, 60ms poll)
- [x] UDP send buffer 2MB, poll interval 20ms
- [x] replaceTrack(null) for camera off

### Stage 3 -- Production hardening (planned)
- [ ] Simulcast (str0m layer selection)
- [ ] Selective forwarding based on subscriber layout
- [ ] SFU clustering (pod pool, NATS signaling between nodes)
- [ ] Recording (Phase 4)
- [ ] Screen sharing with high-res mode

---

## Hub Service

### Auth & identity (done)
- [x] JWT access (30min) + refresh (30day) with rotation
- [x] Login by username OR email
- [x] Refresh endpoint (old revoked, new issued)
- [x] Temp users via magic link (`/join/{token}`)
- [x] Email invite registration (`/register/{invite}`)
- [x] Dev-token fallback for local development

### RBAC & permissions (done)
- [x] Groups with bitfield permissions (READ/WRITE/CONNECT/SPEAK/VIDEO/MANAGE/ADMIN)
- [x] Per-channel permission overrides
- [x] Member management (list, invite, kick)

### Presence (done)
- [x] Session-aware Redis presence (SET with session_id, TTL 60s)
- [x] Refresh uses SET EX (atomic, no GET+EXPIRE race)
- [x] WebSocket presence heartbeat with session_id
- [x] Exponential backoff reconnect (5s -> 120s cap)
- [x] Members-full endpoint with online status

### Storage (done)
- [x] S3 avatar upload (Yandex Object Storage)
- [x] matehub-common shared S3Storage crate
- [x] Per-user avatar authorization check

### Planned
- [ ] Hub settings CRUD (name, icon, description)
- [ ] Channel reordering / categories
- [ ] Audit log
- [ ] Webhook integrations

---

## Chat Service

### Phase 0 -- Data foundation (done)
- [x] ScyllaDB schema: `((hub_id, channel_id, bucket), message_id)` composite partition key
- [x] 7-day bucket window with `channel_buckets` table (tombstone protection)
- [x] DataService with prepared statements, token-aware routing
- [x] Snowflake ID generator (sonyflake)
- [x] NATS fan-out (pub `hub.{id}.channel.{id}.{event_type}`)

### Phase 1 -- Gateway & REST API (done)
- [x] **fastwebsockets** gateway (upgrade from axum::ws, split read/write, writer task)
- [x] Discord-style opcode protocol: HELLO, IDENTIFY, DISPATCH, HEARTBEAT, HEARTBEAT_ACK
- [x] **RESUME** with per-session ring-buffer (512 events / 2MB cap, 5min TTL)
- [x] READY event with session_id on successful IDENTIFY
- [x] RESUMED event after successful replay
- [x] INVALID_SESSION for expired/missing sessions
- [x] Background session reaper (60s interval)
- [x] REST: send message, edit, delete, get history (cursor-based)
- [x] REST: **`POST /v1/sync`** -- per-channel offline catch-up with `limited` flag
- [x] Typing indicator (NATS, no persistence)
- [x] Mark read / unread counters (Redis)
- [x] Rate limiting (5 msg/5s per user/channel via Redis)
- [x] Idempotency (client_id + Redis SETNX)
- [x] Attachments (S3, MIME whitelist, 25MB, Snowflake key)

### Phase 1.5 -- Media pipeline (done)
- [x] Attachment upload endpoint (`POST /v1/channels/{id}/attachments`)
- [x] MIME whitelist: images (jpeg/png/gif/webp), video (mp4/webm + legacy for transcode), audio (mp3/ogg/wav), docs (pdf/txt/zip)
- [x] **Streaming multipart S3 upload** (`S3Storage::upload_stream` in matehub-common)
    - 8 MB part size, single-part fast path for small files, S3 multipart for large
    - Constant ~8 MB RAM per upload regardless of file size; 1 GB max per file
    - Auto-abort multipart on error/oversize; no orphaned uploads
- [x] Image dimension probe (32 MB tee buffer, falls back gracefully for huge images)
- [x] Structured `Attachment` metadata: id, url, content_type, size, width/height, duration, thumb_url, status
- [x] JSON-in-text storage in ScyllaDB `attachments list<text>` (backwards compatible with legacy URL-only rows)
- [x] `ATTACHMENT_UPDATED` gateway event for post-send metadata updates
- [x] Rich message rendering: image gallery + lightbox (prev/next, keyboard, ESC), custom video player with buffered progress bar, inline audio, doc cards with download

### Phase 1.6 -- Video transcoding (done)
- [x] Separate `matehub-transcoder` service (workspace member)
- [x] **NATS JetStream Work Queue** for transcode jobs (`transcode.request` → `transcode.result.{hub_id}`)
    - Queue group for horizontal scaling; `ack_wait=120s` + `max_deliver=3` for redelivery
    - Progress ACK every 30s during long ffmpeg runs (prevents premature redelivery)
- [x] ffmpeg wrapper: libx264 + AAC + `+faststart` (web-streamable mp4)
- [x] ffprobe metadata extraction (width, height, duration)
- [x] Chat service pipeline: upload with `status=transcoding` → enqueue after message write → consume result → update attachment → fanout `ATTACHMENT_UPDATED`
- [x] Frontend transcoding UI states (spinner placeholder, failed state, auto-swap to video player on completion)
- [x] Shared protocol types in `matehub-common::transcode`
- [ ] Production hardening (below)

### Phase 2 -- Mentions, reactions, threads (planned)
- [ ] Server-side mention parsing (`@username` -> user_id resolution)
- [ ] `@everyone` / `@here` rate limiting
- [ ] Mention counters in read_state
- [ ] Reactions table + REACTION_ADD/REMOVE events
- [ ] Threads (thread_root_id + thread metadata table)
- [ ] Kafka durable event log (7-30 day retention)
- [ ] RESUME replay from NATS JetStream (24-72h window)

### Phase 3 -- Scale (planned)
- [ ] Lazy channel subscriptions (windowed member lists, OP 14)
- [ ] Passive sessions (background hubs = messages only, no presence/typing)
- [ ] Relay-shards for hubs >15k online
- [ ] Soft/hard delete with retention policy
- [ ] Audit log in separate partition (180d retention)
- [ ] CDN for attachments (CloudFront/Cloudflare + signed cookies)

### Media pipeline -- production hardening (planned)
- [ ] NATS JetStream `replicas=3` for transcode stream (currently 1 = SPOF)
- [ ] Transactional outbox for `transcode.request` (chat-crash recovery)
- [ ] Zombie reaper cron (attachments stuck `transcoding` > 1h)
- [ ] Parallel part upload in `S3Storage::upload_stream` (FuturesUnordered)
- [ ] Upload-progress WS events (for long GB-scale uploads)
- [ ] Dockerfile for transcoder (debian-slim + ffmpeg preinstalled)

### Phase 4 -- Search & E2EE (planned)
- [ ] Elasticsearch/Meilisearch indexation via Kafka consumer
- [ ] MLS (openmls) for private DM and enterprise secure rooms
- [ ] Dedicated ScyllaDB cluster tooling for enterprise hub migration
- [ ] Multi-region with home-region pinning

---

## TypeScript SDKs (next up)

### Chat SDK (`@matehub/sdk-chat`)
- [ ] EventEmitter core (`eventemitter3` with typed event map)
- [ ] WS connection: HELLO -> IDENTIFY -> READY state machine
- [ ] RESUME / reconnect with decorrelated jitter backoff
- [ ] Heartbeat (41s interval, jittered first send)
- [ ] REST methods: send, edit, delete, history, sync, typing, markRead
- [ ] Dexie IndexedDB cache (messages, channels, outbox, cursors)
- [ ] Offline outbox with UUIDv7 client_id idempotency
- [ ] React hooks (`@matehub/sdk-chat/react`): useSyncExternalStore, TanStack Query

### Video SDK (`@matehub/sdk-video`) -- existing, needs polish
- [x] WebRTC client with trickle ICE
- [x] Mute signaling, speaking detection
- [ ] Reconnect with backoff
- [ ] Simulcast layer preference
- [ ] Screen share API

---

## Frontend

### Auth pages (done)
- [x] `/login` -- email/username + password, remember me
- [x] `/join/[token]` -- magic link landing (resolve, enter hub)
- [x] `/register/[invite]` -- email invite registration

### Hub UI (in-progress)
- [x] Sidebar with channels (voice/text)
- [x] Member sidebar with online status
- [x] Voice channel view with call grid
- [x] Call controls (mute, camera, leave)
- [ ] Text channel view with chat messages
- [ ] Message input with send, typing indicator
- [ ] Message history (infinite scroll, cursor pagination)
- [ ] Unread indicators, mention badges
- [ ] Settings / profile editing

---

## Infrastructure

### Current (dev)
- Docker Compose: PostgreSQL, ScyllaDB, NATS, Redis
- 4 services: hub (3002), chat (3003), video (3001), frontend (3000)
- matehub-common shared crate (S3, JWT auth)

### Planned
- [ ] Kubernetes manifests (Helm charts)
- [ ] CI/CD pipeline (GitHub Actions: cargo test, cargo clippy, next build)
- [ ] Prometheus + Grafana dashboards
- [ ] OpenTelemetry tracing (OTLP -> Tempo/Jaeger)
- [ ] Load testing (k6 / Gatling for WS + HTTP)
