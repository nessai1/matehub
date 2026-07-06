# Phase 2.5 — Desktop-приложение для качественного screen share

> **Status:** research зафиксирован 2026-07-02. Реализован десктоп-клиент
> 2026-07-03/04: оболочка на веб-фронте хаба + нативный медиа-стек —
> screen-share, **HW-энкодеры** (VideoToolbox/NVENC + OpenH264 fallback),
> **системный звук** (Opus) и **нативный войс** (микрофон + приём/микс), CI по
> тегам `desktop/{mac,windows,linux}-*`. Подробности и статус верификации —
> `desktop-share.md`. Впереди: deep-link auth, нативный приём удалённого видео,
> live-верификация медиа на 3 платформах.
> Стыкуется с Phase 3 (см. §7). Наследует use case из `screen-share.md` §1
> (motion-heavy gameplay, 1–5 зрителей, <300ms glass-to-glass, system audio).

---

## 0. TL;DR

Цель — качество шаринга уровня Discord Nitro (1080p60, до ~8 Mbps, «Source»
mode). Ресерч показывает: **качество Discord рождается не в транспорте, а в
нативном пайплайне** — захват через OS API, hardware-энкодер, собственная
битрейт-политика. Discord НЕ шлёт медиа «мимо WebRTC»: у них единый нативный
C++ media engine **поверх WebRTC native library** ([Discord
blog](https://discord.com/blog/how-discord-handles-two-and-half-million-concurrent-voice-users-using-webrtc)),
свой SFU и фиксированный релэй вместо ICE.

Рекомендация: **десктопное приложение на Rust (Tauri), захват через нативные
OS API, hardware encode, публикация в наш существующий SFU через str0m**
(паттерн [BitWHIP](https://github.com/algesten/str0m)). Это снимает все
браузерные потолки качества при нулевых изменениях SFU. Кастомный QUIC-ингест —
отложенная опция (§5.B), она стыкуется с Phase 3, но не является источником
качества.

---

## 1. Жёсткий инвариант: зрители — в браузере

Что бы мы ни сделали с ингестом, **egress к зрителям — WebRTC из нашего SFU**
(MateHub — веб-приложение). Значит:

- «Отдельно от WebRTC» может жить только на отрезке **десктоп → SFU**.
- Кодек обязан декодироваться браузерами зрителей (см. §6.4).
- Наш SFU уже работает в **RTP-passthrough** (`rtp_mode`, byte-identical
  relay, см. `currentstate.md`) — ему всё равно, откуда пришёл RTP-пакет и
  каким битрейтом закодирован payload. Это главный архитектурный актив
  Phase 2.5: SFU кодек- и битрейт-агностик.

---

## 2. Фактчек «как в Discord»

Проверено по [блогу Discord](https://discord.com/blog/how-discord-handles-two-and-half-million-concurrent-voice-users-using-webrtc):

| Легенда | Реальность |
|---|---|
| «Discord шлёт шаринг отдельным протоколом, не WebRTC» | Единый C++ media engine на **WebRTC native library** для desktop/iOS/Android. Браузерный клиент — WebRTC браузера. |
| «Секрет в транспорте» | Секрет в **нативном клиенте**: свой захват, свои энкодеры, свои битрейт-тиры, отсутствие браузерных эвристик. Транспортные отличия — косметика: нет ICE (клиент всегда коннектится к их релэю), своё шифрование, свой signaling вместо SDP. |
| Качество | Free: 720p30 ≈ 1.5 Mbps. Nitro «Stream Quality Boost»: 1080p60 до ~8 Mbps, 4K60 на максимальном бусте. «Source» = нативное разрешение/фреймрейт. |

Вывод: воспроизводить нужно **нативный медиа-пайплайн**, а не изобретать
транспорт. Наш аналог их «WebRTC native library без браузера» — **str0m как
клиентская библиотека** (sans-IO, работает и как клиент: BitWHIP — CLI WebRTC
агент на Rust поверх str0m).

---

## 3. Почему браузерный шаринг упирается в потолок

Браузерный путь (`getDisplayMedia` → `RTCPeerConnection`) мы уже вытюнили
(`screen-share.md` §6: contentHint, maxBitrate 6 Mbps, degradationPreference,
2 simulcast-слоя). Остаточные потолки, которые тюнингом не снимаются:

1. **Дефолтный битрейт-конверт** — без явного `maxBitrate` Chrome держит
   ~2.5–2.6 Mbps на видеопоток; даже с override BWE-эвристики браузера
   консервативны и «прижимают» стрим при малейшем джиттере
   ([multi.app](https://multi.app/blog/making-illegible-slow-webrtc-screenshare-legible-and-fast)).
2. **Нет контроля энкодера**: HW-энкодер не гарантирован (браузер сам решает),
   нет доступа к пресетам (lookahead, AQ, screen-content tools), нет
   гарантированного 60fps захвата полноэкранных источников.
3. **Захват**: `getDisplayMedia` даёт то, что даёт compositор браузера;
   system audio на macOS — только из вкладки; occlusion/HDR/цветовые
   пространства — как повезёт.
4. **Degradation-эвристики**: под нагрузкой CPU браузер сам решает резать
   разрешение/fps, у нас только `degradationPreference` как пожелание.
5. **Один энкодер на трек** в simulcast — параметры слоёв связаны; нативно мы
   можем кодировать слои независимыми энкодерами (например H.264 HW + AV1 SW).

---

## 4. Архитектура Phase 2.5

```
┌────────────────────────── Desktop App (Rust / Tauri) ──────────────────────┐
│                                                                            │
│  Capture (scap)          Encode (HW-first)         Publish                 │
│  ┌──────────────┐        ┌──────────────────┐      ┌───────────────────┐   │
│  │ macOS: SCK   │  BGRA/ │ VideoToolbox     │ RTP  │ str0m (rtp_mode)  │   │
│  │ Win:  WGC    │─NV12──▶│ NVENC / QSV / AMF│─────▶│ ICE/DTLS-SRTP     │──UDP──▶ наш SFU
│  │ Linux: PipeWire       │ fallback: openh264      │ simulcast h/l     │   │      (без изменений)
│  └──────────────┘        └──────────────────┘      └───────────────────┘   │
│  System audio:            Opus (audiopus)               ▲                  │
│  SCK audio / WASAPI        48kHz stereo ────────────────┘                  │
│  loopback / PipeWire                                                       │
│                                                                            │
│  Signaling: тот же WS /ws/{session_id} + JWT (участник-публишер экрана)    │
└────────────────────────────────────────────────────────────────────────────┘
```

Десктопное приложение — **ещё один participant** в существующей сессии со
своим `participant_id`, публикующий screen-track(и). Пользователь остаётся в
браузере (чат, камеры, приём), приложение делает только capture+encode+publish
— как «companion» процесс, спаренный с веб-сессией (авторизация по
deep-link/QR из веб-клиента, `matehub://share?token=...`).

---

## 5. Транспорт десктоп → SFU: два варианта

### A. WebRTC через str0m (рекомендуется как этап 1)

Публикация стандартным WebRTC в существующий SFU. **Ноль изменений на
сервере** — SFU не отличает нативного публишера от браузерного.

Что при этом получаем против браузера:
- полный контроль битрейта (наши тиры: 1080p60 @ 6–8 Mbps по умолчанию);
- гарантированный HW encode + screen-content настройки энкодера;
- независимые энкодеры на simulcast-слои (сегодняшние `h`/`l` из
  `screen-share.md` §6 воспроизводятся 1:1, у SFU уже есть ingress-gate по rid);
- захват стабильных 60fps через нативные API;
- system audio на всех платформах (в браузере на macOS — нет).

Это ровно модель Discord (нативный движок + их SFU), только транспорт у нас
стандартный, потому что SFU уже существует и rtp_mode нам ничего не запрещает.

### B. Кастомный QUIC-ингест (этап 2, опция, стык с Phase 3+)

RTP (или свой фрейминг) поверх QUIC datagrams (`quinn`) → новый listener в
video-сервисе → инжекция в egress через `StreamTx::write_rtp` (rtp_mode
позволяет: форвардеру всё равно, откуда пакет).

Что это даёт СВЕРХ варианта A:
- свой congestion control, тюненный под screen content (держать разрешение,
  жертвовать латентностью до ~0.5–1с — для шаринга допустимо);
- кейфреймы/метаданные по reliable stream (нет «серых квадратов» при потере);
- один QUIC-коннект переживает смену сети (connection migration);
- задел под desktop-зрителей и MoQ.

Цена: свой NACK/FEC/BWE на ингесте, авторизация, маппинг RTP-таймстемпов,
отдельный порт/инфраструктура. [MoQ](https://datatracker.ietf.org/group/moq/about/)
как стандарт ещё не дозрел (draft-18, WG Last Call не объявлен; Safari без
WebTransport в стабильной ветке) — но для **приватного** пути десктоп→SFU
стандарт и не нужен, `quinn` production-grade.

**Решение:** вариант B откладывается до замеров варианта A на плохих сетях.
Источник качества — пайплайн (§3), не транспорт; B оправдан только если
замеры покажут, что WebRTC-путь (NACK/RTX + BWE str0m) деградирует на
реальных сетях буста.

### C. Electron + браузерный движок — отброшен

Это «браузер в рамке»: те же потолки §3, плюс 150MB рантайма. Discord для
медиа его и не использует (медиа — нативный модуль).

---

## 6. Компоненты и выбор технологий

### 6.1 Оболочка: Tauri (Rust)

- Rust-шоп: SFU, будущий load-rig и десктоп-паблишер разделяют код
  (workspace, см. §7).
- UI-поверхность минимальна (пикер источника, индикатор, настройки качества) —
  вебвью Tauri достаточно; медиа-пайплайн полностью в Rust-процессе.
- Альтернатива «чистый native без вебвью» не даёт ничего, кроме
  трёх платформенных UI.

### 6.2 Захват: [`scap`](https://github.com/CapSoftware/scap)

Единая обёртка над ScreenCaptureKit (macOS 12.3+), Windows.Graphics.Capture
(Win10 1903+), PipeWire (Wayland). Продакшен-пользователь — Cap (open-source
Loom). Проверить: fps-стабильность 60, dirty-region hints, HDR→SDR tone-map.
Fallback-план — прямые биндинги (`screencapturekit-rs`, `windows-capture`).

System audio: ScreenCaptureKit audio tap (macOS 13+), WASAPI loopback
(Windows), PipeWire (Linux). В браузере это была самая дырявая часть — здесь
закрывается полностью.

### 6.3 Кодирование: HW-first

| Платформа | Энкодер | Биндинги |
|---|---|---|
| macOS | VideoToolbox (H.264/HEVC) | [`shiguredo/video-toolbox-rs`](https://github.com/shiguredo/video-toolbox-rs) |
| Windows/Linux + NVIDIA | NVENC (H.264/HEVC/AV1) | [`shiguredo/nvcodec-rs`](https://github.com/shiguredo/nvcodec-rs) |
| Windows/Linux + Intel/AMD | QSV / AMF | ffmpeg-биндинги или vendor SDK |
| Fallback (любая) | openh264 / x264 (software) | есть в экосистеме |

Настройки под screen content: zero-lookahead/low-latency пресет, длинный GOP +
кейфрейм по запросу (PLI от SFU), AQ под текст, CBR-подобный рейт-контроль под
наш бюджет. Simulcast: `h` = 1080p60 6–8 Mbps HW; `l` = 540p15 400 kbps
(software, дёшево) — соответствует ingress-gate SFU (rid `h`/`l`).

### 6.4 Кодек на проводе: H.264 → AV1 как апгрейд

- **База: H.264 High** — единственный кодек, который декодируют все зрители
  включая Safari; HW-декод повсеместен.
- **AV1** даёт лучшее качество текста на том же битрейте (screen content
  tools), но приём: Chrome/Edge/Firefox — да; **Safari — только M3+ /
  iPhone 15 Pro+** (HW-декод, софтверного нет). SFU релэит один поток всем →
  AV1 включать только когда все подписчики умеют, либо кодировать двумя
  «слоями» (H.264 + AV1) и выбирать per-subscriber — это ложится ровно на
  механизм step 2 layer-select (§7).

### 6.5 Публикация: str0m

Тот же крейт, что в SFU (workspace dependency!). Клиентский паттерн доказан
BitWHIP. rtp_mode на клиенте: пакетизацию H.264/AV1 делаем сами (RFC 6184 /
AV1 RTP spec) или отдаём str0m sample-mode на publish-стороне (на клиенте
repacketization-бага нет — мы источник, а не релэй). Решить при
прототипировании.

---

## 7. Стыковка с Phase 3

Phase 3 (см. `str0m-analysis.md` §5 + UPDATE в memory `str0m_findings`):
апгрейд str0m 0.7→0.21, layer-select (step 2), пагинация подписок, шардинг
media-тредов, load-rig; гибридный SRTP-пайплайн — только по замерам.

| Phase 3 элемент | Стык с Phase 2.5 |
|---|---|
| **str0m 0.7 → 0.21** | Обязателен ДО десктоп-клиента: клиенту нужны `Arc<[u8]>` write-path, `RtpWrite`-билдер, pluggable crypto (aws-lc-rs вместо OpenSSL — упрощает сборку десктоп-бинаря на 3 платформы). Одна версия str0m в workspace на SFU и клиент. |
| **Step 2 layer-select** | Пререквизит качества: per-subscriber выбор `h`/`l` для screen share (сейчас v1-gate шлёт всем `h`). Расширение: «слой» = не только разрешение, но и кодек (H.264/AV1) от нативного публишера. |
| **Load-rig** | Десктоп-паблишер **и есть** бот для нагрузочного тестирования: тот же crate без UI = headless-паблишер N потоков. Один код — две задачи. |
| **QUIC-ингест (§5.B)** | Если понадобится — это тот же video-сервис, тот же `write_rtp`-вход, что у Phase 3 гибрида; оба требуют инжекции RTP мимо полного Rtc. Делать после замеров, не до. |
| **Шардинг/пагинация** | Не пересекаются: для SFU нативный публишер — обычный участник. |

Предлагаемая структура в репо:

```
crates/
  matehub-rtc-client/   # str0m-обвязка: signaling client, publish pipeline (общая для desktop и load-rig)
apps/
  desktop-share/        # Tauri: UI + capture + encode, зависит от matehub-rtc-client
```

Порядок работ (черновой):
1. str0m upgrade 0.7→0.21 в SFU (Phase 3 шаг 1) — разблокирует всё.
2. Прототип headless-паблишера: scap→openh264→str0m→SFU, файл/тест-паттерн,
   смоук через существующий ROOM_DEBUG.
3. HW-энкодеры (VideoToolbox первым — дев-машины macOS), simulcast h/l.
4. Tauri-оболочка: пикер, deep-link auth из веб-клиента, индикаторы.
5. System audio per-platform.
6. Step 2 layer-select в SFU (Phase 3) → per-subscriber качество.
7. Замеры на плохих сетях → решение по QUIC-ингесту (§5.B).

---

## 8. Открытые вопросы

1. **macOS TCC / нотаризация** — Screen Recording permission, подпись и
   нотаризация бинаря (Apple Developer account). Windows: code signing (SmartScreen).
2. **Пакетизация на клиенте**: str0m sample-mode vs своя RFC 6184 — проверить
   на прототипе (шаг 2), что sample-mode на источнике не портит поток
   (наш SFU-баг был именно в РЕ-пакетизации релэя, источника не касается).
3. **`scap` зрелость**: 60fps stability, multi-display, dirty regions —
   прототип покажет; fallback на прямые биндинги заложен.
4. **AV1 encode стоимость**: SVT-AV1 realtime 1080p60 на средних CPU vs
   NVENC AV1 (только RTX 40xx+) — замерить до обещаний.
5. **Deep-link auth flow**: токен из веб-сессии → десктоп; TTL, revoke при
   leave.
6. **Linux**: Wayland portal UX (пикер системный), X11 legacy — поддерживать ли.
7. Remote control (как в Discord) — сознательно ВНЕ scope Phase 2.5.

---

## 9. Источники

- [Discord: How Discord Handles Two and Half Million Concurrent Voice Users using WebRTC](https://discord.com/blog/how-discord-handles-two-and-half-million-concurrent-voice-users-using-webrtc)
- [Discord Go Live and Screen Share (тиры качества)](https://support.discord.com/hc/en-us/articles/360040816151-Go-Live-and-Screen-Share)
- [multi.app: Making Illegible, Slow WebRTC Screenshare Legible and Fast](https://multi.app/blog/making-illegible-slow-webrtc-screenshare-legible-and-fast)
- [str0m (sans-IO WebRTC, клиентский паттерн — BitWHIP)](https://github.com/algesten/str0m)
- [scap — cross-platform screen capture in Rust](https://github.com/CapSoftware/scap)
- [shiguredo/nvcodec-rs](https://github.com/shiguredo/nvcodec-rs), [shiguredo/video-toolbox-rs](https://github.com/shiguredo/video-toolbox-rs)
- [IETF MoQ WG](https://datatracker.ietf.org/group/moq/about/), [draft-ietf-moq-transport](https://datatracker.ietf.org/doc/draft-ietf-moq-transport/)
- [webrtc.ventures: Should You Still Consider AV1 in WebRTC (2026)](https://webrtc.ventures/2026/04/should-you-still-consider-av1-codec-in-your-webrtc-architecture/)
- Внутренние: `screen-share.md` (браузерный путь, профили качества), `currentstate.md` (RTP passthrough), `str0m-analysis.md` §5 + memory `str0m_findings` (Phase 3)
