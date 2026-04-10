# str0m 0.7 -- анализ рисков и bottleneck'ов

> Результат глубокого исследования исходников str0m (из cargo registry),
> зависимостей (Cargo.lock), и доступной информации о production usage.

---

## 1. Factual findings (подтверждено из исходников и lockfile)

### 1.1 Crypto backend -- OpenSSL, не ring/rustls

str0m 0.7.0 зависит от:
```
openssl = "0.10.76"
openssl-sys = "0.9.112"   # FFI к libssl
```

DTLS handshake и SRTP encrypt/decrypt идут через **OpenSSL** C library.
Это значит:
- Нужен `libssl-dev` при сборке и `libssl` в runtime
- Docker: использовать `debian:bookworm-slim`, **не** Alpine (musl + openssl = боль)
- AES-NI hardware acceleration работает (OpenSSL использует на x86-64)
- **Наша спецификация ошибочно указывает `ring` и `rustls` для SRTP/DTLS** -- нужно исправить

### 1.2 Forwarding path: минимум 2 копии на пакет

```
UDP recv buffer ──copy──> str0m internal buffer ──SRTP decrypt in-place──>
  ──Event::MediaData (owned Vec<u8>)──> наш код ──write_rtp()──>
  ──str0m builds new RTP packet + SRTP encrypt──> Output::Transmit (owned Vec<u8>)
  ──copy──> UDP send buffer
```

На каждого подписчика: **1 аллокация + 1 копия** через `write_rtp()` -> `Transmit`.
Для пакета пересылаемого 9 подписчикам: ~10 аллокаций (1 input + 9 output).

При 500 соединениях с активным видео: **миллионы аллокаций/сек**.

### 1.3 RTX cache -- основной потребитель памяти

RTX cache хранит недавние пакеты для retransmission по NACK.
Дефолт: ~500-1000 пакетов * MTU (~1200 bytes) = **600KB-1.2MB на стрим**.

500 коннектов * 4 стрима (audio + 3 simulcast) = **1.2-2.4 GB** только на RTX.

### 1.4 Полная деpacketизация на hot path

str0m парсит кодек-специфичные заголовки (VP8, VP9, H.264, H.265, Opus) даже когда SFU нужно только пересылать пакеты. Это лишний CPU -- ~5-10% overhead.

Есть `enable_raw_packets(true)` в builder -- возможно обходит деpacketизацию.
Есть модуль `change/direct.rs` -- потенциально SFU-friendly API.
**Требует проверки в коде.**

### 1.5 Rtc не Sync

`Rtc` требует `&mut self` для `handle_input()` и `poll_output()`.
Один `Rtc` instance = один owner thread в любой момент.
Это **не проблема** для нашей архитектуры (dedicated media threads).

### 1.6 Нет встроенного GCC/pacer (противоречие между агентами)

Агент 1: "No built-in congestion control algorithm, no pacer"
Агент 2: Нашёл полный BWE модуль (`bwe/trendline_estimator.rs`, `rate_control.rs`, etc.)

**Вероятное объяснение:** str0m имеет BWE internals, но для SFU-сценария вам нужно
самим решать, как использовать bandwidth estimate для simulcast layer switching.
str0m даёт сигнал "estimated bandwidth = X bps", вы решаете "значит отправлять low layer".

Pacer (anti-burst) -- скорее всего нужно реализовать самим.

### 1.7 Зависимость `combine` тянет tokio транзитивно

```
combine -> bytes, futures-core, pin-project-lite, tokio, tokio-util
```

Используется только для SDP parsing (cold path). Не влияет на runtime,
но раздувает compile time. В sans-I/O библиотеке -- ироничная транзитивная зависимость.

---

## 2. Оценки масштабируемости

| Сценарий | Соединений на 16-core node | Примечание |
|----------|---------------------------|------------|
| Наивное использование str0m | 200-300 | Все копии, дефолтные буферы, полная деpacketизация |
| С тюнингом (RTX cache, raw packets, jemalloc) | 400-600 | Реалистичный Phase 1-2 |
| Гибрид (str0m для signaling/ICE/RTCP, свой SRTP forwarding) | 800-1200 | Phase 3+ оптимизация |

### Что НЕ является bottleneck'ом:
- **SRTP crypto** -- AES-NI даёт 3+ GB/s, при 1 Gbps aggregate media это 20x headroom
- **SDP parsing** -- microseconds, cold path
- **ICE checks** -- cold path, especially в ICE-lite mode
- **Timer management** -- 3-5 таймеров на коннект, min-heap тривиален
- **poll_output() pattern** -- O(1) per output item, не wasted polling

### Что ЯВЛЯЕТСЯ bottleneck'ом:
- **Аллокации на forwarding path** -- миллионы/сек, нужен jemalloc/mimalloc
- **Копии пакетов** -- 2-3 per packet per destination
- **RTX cache memory** -- GB при дефолтных настройках
- **Деpacketизация** -- лишний CPU на parsing кодеков

---

## 3. Риски

### RISK-1 (HIGH): Forwarding path не zero-copy

**Проблема:** str0m проектировался как WebRTC endpoint library, не как SFU forwarding engine.
handle_input() принимает &[u8] (immutable) -> вынужденная копия для SRTP decrypt.
write_rtp() создаёт новый пакет -> аллокация + копия per subscriber.

**Impact:** При 500 коннектах с видео -- миллионы аллокаций/сек.
Потолок ~200-300 коннектов без оптимизации.

**Mitigation (Phase 1):** Использовать jemalloc. Достаточно для 10 участников.
**Mitigation (Phase 2):** Использовать `enable_raw_packets(true)` + `change/direct.rs` API.
Проверить, обходят ли они полную деpacketизацию.
**Mitigation (Phase 3):** Гибридная архитектура -- извлечь SRTP ключи после DTLS handshake,
делать forwarding через свой zero-copy pipeline (ring для SRTP), str0m только для
ICE/DTLS/RTCP/signaling.

### RISK-2 (HIGH): OpenSSL dependency вместо ring/rustls

**Проблема:** Наша спецификация указывает ring + rustls. str0m 0.7 использует OpenSSL.
Два crypto backend'а в одном процессе. Усложнение сборки и Docker images.

**Impact:** Build complexity, runtime dependency на libssl, cross-compilation pain.

**Mitigation:**
- Принять OpenSSL для str0m (DTLS/SRTP)
- ring/rustls использовать для signaling TLS (axum) и потенциально custom SRTP в Phase 3
- Docker base: `debian:bookworm-slim`
- Отслеживать: появится ли rustls backend в будущих версиях str0m

### RISK-3 (MEDIUM): RTX cache memory explosion

**Проблема:** 500 коннектов * 4 стрима * дефолтный RTX cache = 1-3 GB.

**Mitigation:** Настроить RTX cache на 100-200 пакетов (покрывает ~200ms retransmission window).
Для SFU publisher-side RTX cache важнее subscriber-side.

### RISK-4 (MEDIUM): Bus factor = 1

**Проблема:** Единственный активный maintainer -- Martin Algesten.
Pre-1.0, API может ломаться. Если maintainer уйдёт -- форк и поддержка на нас.

**Mitigation:**
- Абстрагировать str0m за trait interface в нашем коде
- Не зависеть от internal APIs str0m, только от stable public API
- Fallback plan: webrtc-rs (дорогая миграция, ~weeks, но возможная)
- Contribute upstream -- чем больше мы вкладываем, тем больше влияния

### RISK-5 (MEDIUM): Browser interop (Safari)

**Проблема:** Safari -- наиболее проблемный браузер для str0m.
SDP quirks, DTLS edge cases. Chrome стабилен, Firefox в основном работает.

**Mitigation:** Интеграционные тесты с headless Chrome, Firefox, Safari (Playwright + WebRTC).
CI pipeline с browser matrix.

### RISK-6 (LOW): Полная деpacketизация

**Проблема:** str0m парсит VP8/VP9/H264 заголовки даже для forwarding.
~5-10% лишнего CPU.

**Mitigation:** Проверить `enable_raw_packets(true)`. Если не помогает --
не критично для Phase 1-2, оптимизировать в Phase 3.

---

## 4. Что нужно исправить в спецификации

| Строка | Было | Должно быть |
|--------|------|-------------|
| 3.1 Crypto (SRTP) | `ring` -- BoringSSL assembly | `openssl` (через str0m) -- AES-NI hardware acceleration. `ring` как fallback для custom SRTP в Phase 3 |
| 3.1 TLS/DTLS | `rustls` -- Integrates with str0m | `openssl` (через str0m) для DTLS. `rustls` для signaling TLS (axum) |
| 4.1.1 SRTP | AES-128-CM (in-place, zero-copy) | AES-128-CM (in-place внутри str0m, но 1 copy на входе из recv buffer) |
| 4.1.3 | GCC implementation | GCC: str0m даёт TWCC signals + internal BWE, но layer switching logic -- наш |
| 11 Cargo.toml | ring, rustls в deps | Оставить (для axum TLS + future custom SRTP), но добавить комментарий что str0m использует openssl |

---

## 5. Рекомендуемая стратегия использования str0m по фазам

### Phase 1 (MVP, до 10 участников)

```
Используем str0m "наивно":
- Один Rtc instance per participant
- enable_raw_packets(true) для forwarding
- ICE-lite mode
- Дефолтные буферы (10 участников = ~400MB, ок)
- jemalloc как global allocator
- OpenSSL принимаем как есть
```

Этого достаточно. 10 участников -- это ~100 пакетов/сек на участника,
~1000 forwarding operations/sec. Даже наивный str0m справляется без напряжения.

### Phase 2 (до 100 участников)

```
Оптимизация в рамках str0m API:
- Тюнинг RTX cache (100-200 пакетов вместо дефолта)
- Проверить change/direct.rs API
- Per-thread Rtc ownership (50-100 Rtc на media thread)
- crossbeam channels для inter-thread forwarding
- Профилирование: perf, flamegraph, heaptrack
- mimalloc вместо jemalloc если лучше для мелких аллокаций
```

### Phase 3 (500+ участников)

```
Гибридная архитектура:
- str0m: ICE, DTLS handshake, RTCP processing, BWE, signaling
- Custom pipeline: SRTP encrypt/decrypt (ring), zero-copy forwarding
- После DTLS completes -- извлечь SRTP master key + salt
- Forwarding path: recv -> slab buffer -> SRTP decrypt (ring, in-place)
  -> RTP header rewrite (in-place) -> SRTP encrypt (ring, in-place)
  -> send (sendmmsg batch)
- str0m продолжает обрабатывать RTCP (NACK, PLI, TWCC) отдельно
```

Это архитектура а-ля "OpenSSL для рукопожатия, потом переключаемся на ring для data path".
Аналог того, как nginx использует OpenSSL для TLS handshake, но может использовать
kernel TLS offload для data transfer.

---

## 6. Открытые вопросы (требуют проверки в коде)

1. [ ] Что именно делает `enable_raw_packets(true)`? Обходит ли деpacketизацию?
2. [ ] Что в модуле `change/direct.rs`? Есть ли SFU-friendly API?
3. [ ] Можно ли извлечь SRTP ключи из str0m после DTLS handshake?
4. [ ] Какой размер RTX cache по умолчанию и как его настроить?
5. [ ] `Rtc` -- Send или !Send? Можно ли перемещать между потоками?
6. [ ] Есть ли feature flag для rustls backend вместо openssl?
7. [ ] Какой overhead на `Event::MediaData` -- owned Vec или borrowed slice?
