# Video Service — Current State (2026-04-18)

> Снимок состояния services/video на момент аудита. Фиксирует что сделано из
> Stage 1 / Stage 2, статус known bugs из `bugs.md`, и perf audit кода `sfu/mod.rs`.
> Этот файл — working doc: обновляется по мере фиксов.

---

## Update (2026-07-02): forwarding → RTP passthrough — DONE, проверено живыми звонками

Медиа-форвардинг переведён со str0m **sample-mode** (`Event::MediaData` +
`Writer::write`) на **RTP passthrough** (`Rtc::builder().set_rtp_mode(true)`,
`Event::RtpPacket` на входе, `StreamTx::write_rtp` на выходе). Контрольный
звонок c4896cd7: 30fps стабильно, `packetsLost:0`, `nackCount:0`, `pliCount:0`.

**Почему:** sample-mode депакетизировал и **заново пакетизировал** VP8/H264 на
egress, ломая межкадровые ссылки → кросс-браузерные артефакты декодирования при
нулевой потере пакетов. RTP passthrough релэит payload издателя
**byte-identical**.

**v1 — single-layer.** Полная история миграции (8 слоёв багов, снятых по
ROOM_DEBUG-звонкам) — в `rtp-passthrough-handoff.md`. Инварианты, на которых
стоит форвардинг (`sfu/mod.rs::forward_rtp` + event loop):

- **PT ремап** через `codec_config().match_params()` — каждый Rtc негоциирует
  PT независимо; запись с чужим PT → str0m молча дропает («Media is missing
  PT») = чёрное видео.
- **Seq offset-rewrite** (`TrackOut::egress_seq_base`):
  `egress = START + (ingress_ext_seq − base)`. Сохраняет порядок и
  относительные позиции (str0m отдаёт пакеты в порядке ПРИБЫТИЯ, де-RTX'нутые
  резенды приходят поздно — счётчик по приходу ломал сборку кадров).
- **`StreamTx::set_unpaced(true)`** перед записью — дефолтный leaky-bucket
  пейсер душит релэй без BWE-рейта.
- **Header extensions НЕ пробрасываются** — MID/RID/TWCC/abs-send-time
  транспортно-скоуплены; протухший MID издателя после `remote_acked_ssrc`
  заставлял demuxer Chrome перепривязывать SSRC → видео умирало через ~1с.
  Копируются только end-to-end: audio_level, voice_activity,
  video_orientation, video_content_type.
- **Poll-контракт str0m** (два бага contract violation):
  (а) `poll_output` — консюмящая очередь; дренаж, выбрасывающий
  `Output::Event`, съедает чужие RtpPacket/KeyframeRequest → форвардинг
  пишет в `pending_flush`, дренируемый через полный обработчик
  (`flush_pending_writes`); (б) rtp_mode держит входящий пакет в
  **однослотовом** `pending_packet` — обязателен poll до Timeout после
  КАЖДОГО `handle_input` (`poll_target` сразу за `handle_command`), иначе
  батч датаграмм затирает сам себя (терялось ~2/3 медиа, невидимо для
  RR/NACK).
- PLI идёт через `StreamRx::request_keyframe` (`direct_api`), не Writer;
  троттлится (`request_keyframe_throttled`).

**Что осталось:**
- `pkt.payload.clone()` на каждого подписчика (bug #4 ниже) → Phase 2:
  `Arc<[u8]>`.
- Simulcast layer-select (`selected_rid`/`next_layer` живут, но в v1 форвард их
  не применяет) → step 2: per-subscriber выбор SSRC + RTP munging.
- SDK: публиковать треки через `addTransceiver(direction:'sendonly')` — сейчас
  join-ответ делает publish-m-line'ы sendrecv → фантомные ontrack у клиента
  (безвредны, SDK их игнорит).
- Закрыт open-question #1 из `str0m-analysis.md`: passthrough это
  `set_rtp_mode(true)`, а **не** `enable_raw_packets`.

---

## 1. Что реализовано фактически

### Stage 1 (MVP) — DONE
- `POST /v1/sessions` с идемпотентным создаванием по `channel_id`
- `GET /v1/sessions/{sid}` с участниками
- `GET /health`
- WebSocket signaling `/ws/{session_id}` (JSON messages)
- SFU engine на str0m, full ICE (не ICE-lite — см. комментарий в `handle_join`)
- DTLS-SRTP через OpenSSL (str0m дефолт)
- Audio + video forwarding
- Session lifecycle: create on first join, destroy on last leave
- Participant_joined / participant_left broadcast
- Server-side renegotiation при join/leave (добавление/удаление Outgoing tracks)
- Zombie detection: ICE disconnected + 30 сек без UDP → `rtc.disconnect()`
- `mute_changed` signaling + broadcast `participant_muted` (pers state в AppState)

### Stage 2 — NOT STARTED
Ни один пункт из `stage-2.md` не начат: нет simulcast, screen share (через frontend
уже ходит, но без SDP simulcast), NACK tuning, TWCC, dedicated media thread,
TURN, JWT auth, Redis, NATS, Prometheus, архитектурных закладок.

### Файловая структура
```
services/video/src/
├── main.rs              73  entry point, UDP bind, socket buffers 2MB
├── config.rs            54  env config
├── state.rs             89  AppState (под parking_lot::Mutex) + REST DTO
├── lib.rs                5
├── api/
│   ├── mod.rs           14
│   ├── sessions.rs     100  POST/GET sessions
│   └── ws.rs           252  WebSocket handler
├── sfu/
│   ├── mod.rs          852  SfuEngine, poll loop, forwarding
│   ├── session.rs       77  SfuSession, SfuParticipant, TrackIn/Out
│   └── udp.rs            2  (пусто)
└── signaling/
    ├── mod.rs            2
    └── messages.rs      71  ClientMessage/ServerMessage enums
```
Всего 1591 строка Rust.

---

## 2. Known bugs — актуальный статус

### BUG-1 Growing delay (CRITICAL) — **FIXED IN CODE, UNVERIFIED UNDER LOAD**

Исходно: `poll_all_outputs()` двухфазно собирал MediaData в Vec и потом рассылал,
без `poll_output()` между write'ами → пакеты копились в SRTP-очереди str0m,
delay рос до секунд.

**Текущее состояние:** `sfu/mod.rs:552-695` переписан на interleaved архитектуру.
`poll_all_outputs` теперь идёт по participants индексно, на `Event::MediaData`
вызывает `forward_media_now()`, который после каждого `writer.write()` крутит
`drain_transmits` до `Timeout` (строки 747-758). Это соответствует str0m chat
example.

**Надо проверить:** двусторонний звонок 5+ минут без роста delay. Tests не
написаны. Stage 2 фаза A пункт P-1 считается закрытым в коде, но не валидирован.

### BUG-2 Stream ID mapping (MEDIUM) — **PARTIALLY FIXED**

Исходно: фронт не мапил `stream.id` от str0m к `participantId`, второй участник
не видел первого.

**Текущее состояние:** backend в `negotiate_pending_tracks` (строки 784-799,
815-819) передаёт `TrackMapping { stream_id, participant_id, user_id }` через
`ServerMessage::Offer { tracks }` и использует `track.origin.to_string()`
как stream_id в `add_media()`. То есть SFU правильно нумерует streams.

**Что проверить:** фронтендовый SDK использует поле `tracks` из offer для
маппинга `stream.id → participant_id` в `ontrack` handler. Если да — BUG-2
закрыт, если нет — нужен фикс на фронте. Не проверял.

### BUG-3 ICE Disconnected after Completed (LOW) — **HANDLED**

Не дисконнектим участника на `IceConnectionState::Disconnected` — только ставим
флаг `ice_disconnected: true` в `poll_all_outputs` (строка 598-602). Реальный
cleanup идёт через zombie detection в `tick()`: если `ice_disconnected &&
last_activity_at.elapsed() > 30s` — `rtc.disconnect()`. Это правильное
поведение для Stage 1.

### Новых багов не зафиксировано
Но см. Раздел 3 — perf issues и architectural smells, которые НЕ баги, но
взорвутся под нагрузкой.

---

## 3. Performance audit (2026-04-18)

Приоритет: от самых взрывоопасных к косметике. Номера — для ссылок в коммитах
и task list.

### Критично (Stage 2 фаза A)

#### #1 — Control plane и media plane в одном tokio task

`SfuEngine::run` (строки 90-154) тянет через `tokio::select!` UDP recv,
WS commands, timer tick и вдобавок `poll_all_outputs()` после каждой ветки.
Один tokio worker конкурирует media forwarding с signaling work.

**Последствие:** не выполняем spec SLA `< 1ms p99 packet forwarding latency`.
На любой всплеск activity (bulk renegotiation, join storm) audio/video
глотает jitter.

**Лечение:** `stage-2.md` пункт 2.6 — вынести engine в `std::thread::spawn`
с blocking `std::net::UdpSocket`, коммуникация через `crossbeam::channel`.

**Blocks:** #2, #3, #4, #5 (разные фиксы в разных слоях, но общая цель).

#### #2 — O(sessions × participants) UDP demux

`handle_udp_packet` (строки 478-508) линейно обходит все sessions × participants
и вызывает `rtc.accepts(&input)` на каждом.

Math на 500 участниках:
- ~50 входящих pps/участник × 500 = 25k pps
- до 500 `accepts()` вызовов на пакет
- 12.5M `accepts()`/сек только на демультиплексацию

Взорвётся уже на 20-30 активных участниках.

**Лечение:** `HashMap<SocketAddr, (SessionId, ParticipantId)>` в
`SfuEngine`. После первого успешного `accepts() && handle_input() ok` —
сохраняем mapping. Fallback на linear scan для первого пакета от нового
remote addr (STUN binding requests). Инвалидация при leave/disconnect.

#### #3 — `poll_all_outputs()` вызывается после КАЖДОЙ select-ветки и ходит по всем

`run()` после каждого события (один UDP пакет, одна команда, один тик)
вызывает `poll_all_outputs()`, который O(N) по всем participants во всех
sessions.

Если пакет пришёл от Alice — Bob/Charlie/Dave поллятся просто так, их state
не менялся. Внутри `negotiate_pending_tracks` — снова O(N×K) по всем треках,
50 раз в секунду на говорящего.

**Лечение:**
- Target poll только того participant'а, что получил input
- Флаг `session.has_pending_negotiation: bool`, поднимается на MediaAdded/Leave,
  сбрасывается после batch-offer

#### #4 — `data.data.clone()` на каждый subscriber в fan-out

`forward_media_now:741`:
```rust
writer.write(pt, data.network_time, data.time, data.data.clone())
```

На N подписчиков — N клонов payload. Room 10 = 9 клонов каждого RTP пакета.
Spec кричит про zero-copy forwarding и slab allocator.

**Лечение (краткосрочно):** `Arc<[u8]>` + slab pool, клон только на API boundary
str0m (он требует `Vec<u8>` owned).

**Лечение (долгосрочно, Phase 3):** hybrid pipeline в обход str0m для forwarding
с SRTP rekey через ring.

#### #5 — Vec allocation в forward_media_now на каждый media packet

Строка 707: `let targets: Vec<(ParticipantId, Mid)> = { ... };` — собирается
заново на каждый `MediaData` event. 50 аудио pps × N говорящих = тысячи
heap allocs/сек.

**Лечение:** Pre-computed `Session.forwarding_map:
HashMap<(source_pid, source_mid), SmallVec<[(target_pid, target_mid); 8]>>`.
Обновляется при join/leave/track-open/track-close. В hot path — O(1) lookup.

### Болит под нагрузкой (Stage 2 фаза B)

#### #6 — `recv_from` один пакет за syscall

Нет `recvmmsg`/`sendmmsg` (spec требует). На Linux — io_uring. Пока pps < 10k —
не критично. При 100+ conn на ноду — душит 20-30% CPU в user-kernel transitions.

#### #7 — Дефолтный glibc malloc

Нет `#[global_allocator]`. jemalloc/mimalloc дают стабильные 10-20% throughput
на high-pps нагрузках. Spec рекомендует.

#### #8 — `interval` с дефолтным `MissedTickBehavior::Burst`

`main.rs` / `sfu/mod.rs:94` — `interval(Duration::from_millis(20))` без настройки.
Под load spike даёт burst из `tick()`-вызовов, каждый O(N). Надо `Skip`.

#### #9 — Двойной source-of-truth для participants

`AppState::sessions` (под `parking_lot::Mutex`, для REST) и `SfuEngine::sessions`
(в engine task, для media). Два набора participant state:
- Гонка при join: REST видит state=Connecting раньше, чем SFU знает участника
- Двойной broadcast на join/leave
- Blocking `parking_lot` lock из async контекста в `handle_ws` — contention
  point на 100+ conn/sec

**Лечение:** engine — single writer. REST читает snapshot через
command/query channel или через `tokio::sync::watch`.

#### #10 — Broadcast-ы в handle_ws под mutex

Строки 82-83 и 239-241: `for p in session.participants.values() { p.ws_tx.send(...) }`
под `state.inner.lock()`. `send()` быстрый, но lock держится на весь цикл.
При 100-участниковой комнате и join/leave storm — serialization point.

### Косметика / долг (пока не болит)

#### #11 — `TrackMapping.clone()` per offer

`negotiate_pending_tracks:841`: `Some(track_mappings.clone())`. При N
одновременных offer'ах на одну комнату — N × (N-1) клонов metadata.

#### #12 — Hot-path `tracing::info!` с аллокацией String

`api/ws.rs:127`:
```rust
tracing::info!(%participant_id, raw = %text.chars().take(120).collect::<String>(), "WS raw message");
```
`chars().take(120).collect::<String>()` — аллокация на каждое WS сообщение,
включая trickle ICE candidates (десятки на join). Аналогично
`handle_ice_candidate:377` — `info!`, хотя это hot path.

**Лечение:** понизить до `debug!` либо использовать `tracing`-ленивые форматтеры.

#### #13 — Unbounded WS channel без backpressure

`mpsc::unbounded_channel()` для ws_tx. Медленный клиент = копится в памяти,
нет drop-oldest policy. На broadcast событиях (speaker change при 100 уч.) —
быстрая раздутая RSS.

#### #14 — `send_task` per participant

На 500 участников — 500 tokio tasks на WS write. Дешёвые, но `ws_sender.send().await`
под залипшим TCP writer = зависание task = накопление в ws_rx без bound.

---

## 4. План фиксов (порядок реализации)

Последовательность выбрана по: (а) безопасность change'а, (б) видимый impact,
(в) unblock-эффект на следующие фиксы.

1. **#2 — O(1) UDP demux.** ✅ DONE (2026-04-18). Добавлен
   `SfuEngine.addr_to_participant: HashMap<SocketAddr, (SessionId, ParticipantId)>`.
   `handle_udp_packet` работает по схеме fast path (O(1)) → slow path (linear +
   populate cache). Инвалидация в `handle_leave` (per-participant retain) и при
   destroy сессии (per-session retain). Stale mapping при ICE restart падает на
   slow path.
2. **#3 — Targeted poll.** ✅ DONE (2026-04-18).
   - `poll_all_outputs` разбит на `poll_participant` / `poll_session` / `poll_all`.
   - `handle_*` возвращают `Option<PollTarget>` (One/Session). UDP demux тоже
     возвращает `Option<(SessionId, ParticipantId)>`.
   - `run()` переведён на targeted poll: UDP → `poll_participant`; Command →
     целевая сессия/участник; Tick → `poll_all`.
   - Добавлен `has_pending_negotiation: bool`. `negotiate_pending_tracks()`
     вызывается только по флагу. Флаг поднимается в `handle_join` (при
     existing_tracks) и в `Event::MediaAdded` (у других участников), сбрасывается
     в начале негоциации, re-raises если кто-то skipped из-за pending_offer.
3. **#5 — Pre-computed forwarding map.** ✅ DONE (2026-04-18).
   - `SfuSession.forwarding_map: HashMap<(pid, mid), Vec<(pid, mid)>>` —
     precomputed fan-out, обновляется incrementally.
   - Populated в `handle_answer` при переходе TrackOutState::Negotiating → Open.
   - Cleanup в `handle_leave` через `SfuSession::drop_from_forwarding(pid)` —
     удаляет publisher keys и subscriber entries в values за один проход.
   - `forward_media_now` теперь делает O(1) lookup через `mem::take` +
     put-back, чтобы итерировать Vec без borrow на HashMap и без clone.
     Zero-alloc на hot path пока entry существует в map.
4. **#8 — MissedTickBehavior::Skip.** ✅ DONE (2026-04-18). В `run()`:
   `interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip)`.
5. **#7 — mimalloc.** ✅ DONE (2026-04-18). `mimalloc = "0.1"` в
   `services/video/Cargo.toml`, `#[global_allocator] static GLOBAL: mimalloc::MiMalloc`
   в `main.rs`. Выбран mimalloc (не jemalloc) — pure Rust, без ffi-деп системы,
   upside ~равный для SFU workload.
6. **#12 — Hot-path logging.** ✅ DONE (2026-04-18).
   - `api/ws.rs` "WS raw message" → debug, `bytes = text.len()` вместо
     `chars().take(120).collect::<String>()`. Preview для failed-parse осталась
     на warn с правильной семантикой (редкий error path).
   - `api/ws.rs` "WS: forwarding ICE candidate" → debug.
   - `sfu/mod.rs` "received remote ICE candidate" → debug.
   - Остальные info! проверены — все по редким событиям (join/leave/state change).
7. **BUG-2 проверка.** ✅ CODE REVIEW DONE (2026-04-18).
   - Backend: `stream_id = origin.to_string()` используется одинаково в SDP
     msid (`add_media`) и в TrackMapping (`ServerMessage::Offer.tracks`).
   - Frontend (`packages/sdk-video/src/client.ts`):
     - `participant_joined` → `this.participants.set(participantId, ...)`
     - `offer.tracks[]` → при `stream_id != participant_id` индексирует participant'а
       и под stream_id (строки 314-327)
     - `ontrack` → `this.participants.get(stream.id)`
   - Поскольку backend использует один UUID для stream_id и participant_id,
     простой lookup по participantId работает. Код выглядит корректно.
   - **Нужен интеграционный smoke на двух браузерах** чтобы подтвердить на живой
     сессии — не могу сделать из кода.
8. **#1 — Dedicated media thread.** ✅ DONE (2026-04-18).
   - `crossbeam::channel` для командного канала WS tokio tasks → media thread
     (tokio::sync::mpsc нельзя poll из sync OS thread).
   - `udp_socket: Arc<tokio::net::UdpSocket>` → `Arc<std::net::UdpSocket>`.
     `SO_RCVTIMEO = 20ms` ставится один раз при старте; даёт детерминированный
     tick без второго тикера.
   - `run() async` → `run_blocking()` sync: `recv_from` blocking → drain
     `cmd_rx.try_recv()` → `Instant::now() >= next_tick` → `tick() + poll_all()`.
     Никаких `.await` yield-point'ов между «прочитал RTP» и «отправил RTP».
   - `main.rs`: `std::thread::Builder::new().name("sfu-media").stack_size(2MB)
     .spawn(|| engine.run_blocking())`. Handle хранится через `mem::forget`
     (thread живёт до process exit).
   - Reply path к клиенту остался через `tokio::sync::mpsc::UnboundedSender`
     (его `send()` sync, thread-safe — media thread пишет, tokio async task
     читает без блокировок).

9. **Tests.** ✅ DONE (2026-04-18).
   - 11 unit tests в `sfu::session::tests` и `sfu::tests`:
     `new_session_is_empty`, `drop_from_forwarding_*` (5 cases: publisher key,
     subscriber entry, empty prune, dual role, unknown no-op),
     `track_out_open_mid_is_some_only_when_open`,
     `stream_id_for_*` (distinct a/v, msid-safe chars (RFC 7941),
     deterministic, unique per publisher).
   - `cargo test -p matehub-video --lib` → 11 passed.

10. **Benchmarks.** ✅ DONE (2026-04-18).
    `benches/sfu_forward.rs` (Criterion), 4 groups:

    | bench | small N | large N | ratio |
    |---|---|---|---|
    | `forwarding_lookup/map_get` | 26ns @1 | 22ns @200 | O(1) |
    | `forwarding_lookup/linear_scan` | 25ns @1 | 791ns @200 | O(N) |
    | `drop_from_forwarding/room_size` | 286ns @5 | 44µs @100 | O(N²), leave path |
    | `addr_demux/hashmap_get` | 29ns @5 | 28ns @500 | O(1) |
    | `addr_demux/linear_scan` | 2ns @5 | 33ns @100 | O(N), crossover ~100 |
    | `stream_id_format` | 95ns | — | baseline |

    Ключевые take-away:
    - На 200 subscribers fan-out lookup **36× быстрее** (22ns vs 791ns).
      Это ровно то что даёт pre-computed forwarding_map.
    - Addr demux: HashMap стабилен на любом N; linear scan пересекается с
      HashMap около 100 участников. Но в реальности `rtc.accepts()` дороже
      чем просто сравнение адресов — бенч недооценивает выигрыш HashMap'а
      в проде.
    - `drop_from_forwarding` O(N²) по room size — приемлемо на leave-path
      (редкий, не hot path).

**Остаётся дальше (Stage 2 фаза B и далее):**
- #4 (zero-copy через Arc<[u8]>)
- #6 (recvmmsg / io_uring)
- #9 (единый source of truth)
- #10 (убрать broadcast под mutex)
- Simulcast, TURN, JWT, Redis, NATS, Prometheus, архитектурные закладки A-1..A-6

---

## 5. Verification checklist

После каждого фикса запускать минимум:
- [ ] `cargo build -p matehub-video` чистый
- [ ] `cargo clippy -p matehub-video -- -D warnings`
- [ ] Двусторонний звонок 2 минуты (smoke), слышно+видно
- [ ] Third participant join/leave, остальные продолжают видеть друг друга

После фиксов #1-#5:
- [ ] Звонок 10 минут, delay не растёт (BUG-1 не regressed)
- [ ] 4 участника одновременно, CPU на idle forwarding стабильный
