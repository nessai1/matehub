# Video Service — Profiling Guide

> Как измерить, где реально упирается video-service под нагрузкой,
> и как доказать (или опровергнуть) гипотезы из ревью — в первую очередь
> вопрос про `data.data.clone()` в `forward_media_now` (B1 в ревью).

---

## TL;DR

```bash
# 1. Build with debug symbols + profiling endpoint
cargo build --release --features profiling -p matehub-video

# 2. Run with realistic settings
MEDIA_SHARDS=4 RUST_LOG=info ./target/release/matehub-video

# 3. In another shell: capture a 30s flamegraph during peak load
curl 'http://localhost:4000/debug/pprof/profile?seconds=30' > sfu.svg
xdg-open sfu.svg
```

Чтобы цифры были осмысленные — нужен трафик. Без 10+ участников SFU
проводит большую часть времени в `recv_timeout` и flame-граф тебе ничего
не покажет.

---

## 1. Нужны debug-символы

В `Cargo.toml` workspace добавь, если ещё нет:

```toml
[profile.release]
debug = true       # symbols, не downgrade оптимизаций
lto = "thin"
codegen-units = 1  # лучшая stack inlining видимость в perf
```

Без `debug = true` flame-граф будет полон `<unknown>` фреймов и в нём
не разглядишь `data.data.clone()` от `match_params` от `mi_malloc`.

---

## 2. Что считать «нагрузкой»

Чтобы B1 имел шанс быть проблемой, нужно достаточно `forward_media_now`-ов.
Грубая прикидка из ревью: 30 человек × 30 fps × 29 подписчиков ≈ 26k pps
fan-out, ~30 МБ/сек RTP-payload allocations.

Минимальный сценарий для значимого профайла:

- **10+ реальных Chrome-вкладок** через `about:webrtc` или synthetic peers.
  Простой вариант — открыть 10 окон Chromium с автозаходом в один канал
  через тестовый аккаунт.
- **5 минут устойчивой работы** — кэши прогреваются, jitter buffers
  стабилизируются, BWE settle в стационарное значение.

Альтернатива: synthetic load gen на pion (Go) или на самом str0m. У нас
есть `services/video/benches/sfu_forward.rs` — он измеряет HashMap fan-out
изолированно, без Rtc/SRTP. Ставит верхнюю границу на «pure data structure»
расход. Гонять `cargo bench -p matehub-video --bench sfu_forward`.

Для проверки `data.data.clone()` именно в hot-pathе — нужен живой Rtc
с SRTP, потому что bench его обходит.

---

## 3. Где смотреть

### A. In-process flamegraph через `/debug/pprof/profile`

Самый удобный путь. Сборка с `--features profiling`, эндпойнт активен,
сэмплинг через `pprof-rs` (стандарт для Rust сервисов).

```bash
# captures 30s, returns SVG
curl 'http://localhost:4000/debug/pprof/profile?seconds=30&frequency=99' \
  > /tmp/sfu.svg
```

В SVG можно мышью наводить, видно процент времени. Ищи:

| Стек | Что значит |
|------|------------|
| `forward_media_now` | Сколько времени горячий путь съедает |
| `mi_malloc` / `mi_free` (или `malloc`) | Сколько на allocator |
| `<Vec<u8> as Clone>::clone` под `forward_media_now` | **Это и есть B1** |
| `srtp_protect` / `aes_*` | SRTP encrypt — обычно крупнейшая статья |
| `recv_timeout` / `epoll_wait` | Idle wait — нормально, если не доминирует |

### B. `perf` (Linux) — для precision и off-CPU

`pprof-rs` хорош, но это user-space sampler. Для off-CPU (блокировки,
syscalls) или kernel time нужен `perf`:

```bash
# Set perf_event_paranoid to allow user perf
sudo sysctl kernel.perf_event_paranoid=1

# Capture 30s
perf record -F 999 -g -p $(pidof matehub-video) -- sleep 30

# Inspect
perf report          # interactive
perf script | inferno-flamegraph > /tmp/sfu.svg   # flamegraph SVG
```

`inferno-flamegraph` — Rust-нативный аналог `flamegraph.pl`:
`cargo install inferno`.

### C. `samply` — самый дружелюбный CLI

```bash
cargo install samply
samply record -p $(pidof matehub-video)
# CTRL-C через 30s — открывает Firefox profiler в браузере
```

Преимущество — Firefox Profiler показывает stacks с тёплой раскладкой и
позволяет переключаться "inverted call tree" / "stack tree" одним кликом.
Идеально для разглядывания malloc.

---

## 4. Как доказать B1 (или опровергнуть)

Пробег для конкретно `data.data.clone()` в `forward_media_now`:

### Шаги

1. Запустить service с `--features profiling`.
2. Засадить 10+ Chrome-вкладок в один voice-канал, **обязательно с camera ON**
   (без видео ничего не клонируется в hot-path — только аудио, payload <300B).
3. Дождаться, пока в `media forwarding stats` логе появится стабильный
   `video_pps`. Должно быть >5000 pps на сессию.
4. Захватить flamegraph: `curl ...?seconds=60 > sfu.svg`.

### Что искать

В дереве под `forward_media_now`:

- **Если суммарный allocator-time (`mi_malloc` + `mi_free`) > 5%** — B1
  реальная проблема.
- **Если ~1-2%** — micro, profile-driven optimization не оправдан.
- **Если allocator не виден** (< 0.5%) — `data.data.clone()` неактуально,
  закрыть тикет.

### Дополнительные подтверждения

```bash
# Memory throughput через RSS / page faults
pidstat -r -p $(pidof matehub-video) 1 30

# malloc вызовы в секунду через bcc tools (нужен root)
sudo execsnoop-bpfcc -f matehub-video    # syscall-level
sudo memleak -p $(pidof matehub-video)   # alloc/free hot stacks
```

Если `pidstat -r` показывает > 100 МБ/с в RSS-deltas, мы выделяем под
hot-path много, и mimalloc справляется только за счёт thread-local
arenas. Это сигнал, что `data.data.clone()` — реально проблема.

### Как фиксить (если подтвердилось)

Проблема в str0m API: `MediaWriter::write` берёт `Vec<u8>` (move). Чтобы
делиться буфером между подписчиками, надо либо:

1. **PR в str0m** — добавить `write_bytes(...)` принимающий `Bytes` (или
   `Arc<[u8]>`). У SRTP контекстов внутри Rtc свои буферы, но входящий
   payload можно ref-counted'ить.
2. **Workaround**: передавать `Arc<Vec<u8>>` и при write делать `(*arc).clone()`
   — НЕ помогает (`clone` всё равно делает `Vec`).
3. **Pool of recyclable buffers** — на каждый `MediaData` арендовать буфер
   из пула, после fan-out возвращать. Сложнее, но избегает heap-trafic.

Реалистичный путь — № 1 в str0m upstream.

---

## 5. Базовые baseline-цифры

Когда есть профайл, сравни с тем, что должно быть нормально для текущего
(post-A1, post-A4) кода:

| Стек / показатель | Ожидаемая доля | Если выше — копать |
|-------------------|----------------|--------------------|
| `srtp_protect` + AES | 30-50% | Это нормально, hot path в WebRTC |
| `forward_media_now` (без вложенных) | 5-15% | Если 30%+ — `data.data.clone()` или HashMap |
| `recv_timeout` + idle | 30-60% | Ниже — недогруз профилировки, выше — bottleneck в чём-то ещё |
| `mi_malloc` / `mi_free` | <3% | 5%+ → B1 |
| `negotiate_pending_tracks` | <1% | >5% — нужно дебоунсить (F4 done, но проверить) |

---

## 6. Continuous monitoring (для будущей жизни)

Два метрика-сигнала, по которым можно alert'ить без flame-графа:

- `matehub_video_udp_send_drops_total` — растёт при kernel send buffer
  переполнении (см. A2). Пограничный сигнал «SFU не успевает».
- `matehub_video_panics_caught_total` — любой ненулевой это инцидент.

И производное:
- Active participants = `matehub_video_participant_joins_total` − `_leaves_total`.
- Per-shard load распределение пока не выводим как метрику; если станет
  актуально — добавить counter с `shard` label из `SfuShard::run_blocking`.
