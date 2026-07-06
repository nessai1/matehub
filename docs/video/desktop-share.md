# Desktop-приложение MateHub (Phase 2.5)

> **Status:** полноценный десктоп-клиент (оболочка на веб-фронте хаба) +
> нативный медиа-стек: screen-share (scap→HW/SW H.264→SFU), захват
> системного звука (Opus) и **нативный войс** (микрофон + приём/микс/
> воспроизведение) в обход WebRTC webview. HW-энкодеры: VideoToolbox (macOS),
> NVENC (Win/Linux, feature-gated), OpenH264 fallback. Deep-link auth и
> приём удалённого видео нативно — следующие этапы §7.
>
> **Проверено:** компиляция всех путей (VideoToolbox — на dev-Mac, NVENC — в
> CI), юнит/loopback-тесты транспорта и DSP. **Не проверено вживую:** сам
> захват+кодек+доставка в реальном звонке и качество войса на 3 платформах —
> нужен live-прогон (ROOM_DEBUG + многосторонний звонок с железом).

## Что это

Десктоп-клиент по «Discord-модели»: **интерфейс — тот же веб-фронт хаба**
(логин/SSO, список хабов и каналов, чат, войс — ровно то, что в браузере),
загружаемый в webview; а **screen-share — нативный медиа-движок на Rust**
(scap → OpenH264 → str0m → SFU), который снимает браузерные потолки качества
(`phase-2.5` §3) и работает мимо `getDisplayMedia`.

Почему так, а не «весь UI нативно» и не «WebRTC целиком в webview»:

- Веб-фронт — один код на браузер и десктоп, всегда актуальный; логин/SSO,
  чат, войс переиспользуются 1:1, cookies webview персистентны на всех трёх
  платформах (WKWebView/WebView2/webkit2gtk — из коробки).
- WebRTC внутри системного webview ненадёжен: WebView2 (Windows) тянет,
  WKWebView (macOS) — с оговорками, **webkit2gtk (Linux) фактически сломан**
  (wry не включает `enable-media-stream`/`enable-webrtc`, дистрибутивные
  сборки экспериментальны). Поэтому качественный шаринг — нативный, тем же
  паттерном, что и SFU (str0m). Войс в webview остаётся браузерным там, где
  платформа его тянет (детали и ссылки — в decision log ниже).

## Состав

```
crates/matehub-rtc-client/    # str0m-обвязка: signaling + publish + recv (workspace member)
apps/desktop-share/           # Tauri-приложение (ВНЕ cargo workspace)
  ui/                         # локальный пикер хабов (статический webview)
  src-tauri/
    src/lib.rs                # окно, мост, команды (share+voice), runtime-ACL
    src/bridge.js             # init-script: window.__MATEHUB_NATIVE__
    src/hubs.rs               # MRU-список хабов (JSON в app config dir)
    src/capture.rs            # scap (видео BGRA + системный звук PCM)
    src/encoder/              # VideoEncoder: videotoolbox / nvenc / openh264 + yuv
    src/audio.rs              # системный звук → Opus 20мс
    src/dsp.rs                # общий DSP (resample, dBov, mix)
    src/voice.rs             # нативный войс: cpal mic + приём/микс/воспроизведение
    src/pipeline.rs           # capture→encode→publish (video+audio)
    Info.plist                # NSMicrophone/NSCamera (mic в cpal + войс в WKWebView)
frontend/lib/native-bridge.ts # типизированный контракт моста (version 1)
frontend/hooks/use-native-screen-share.ts
.github/workflows/desktop-build.yml   # сборки по тегам desktop/{mac,windows,linux}-*
```

`apps/desktop-share/src-tauri` — **отдельный cargo workspace** (корневой
`Cargo.toml` его `exclude`'ит): scap/openh264/tauri тянут платформенные
системные зависимости (ScreenCaptureKit/PipeWire/WebKitGTK), которым нечего
делать в CI сервисов. str0m остаётся единой версии: `matehub-rtc-client` —
path-зависимость из корневого workspace, его тесты гоняет обычный
`cargo test --all`. У приложения свой `Cargo.lock` (коммитится).

## Поток работы

```
[пикер хабов]  tauri://localhost — ui/, capability "default" (core + hub-мгмт)
      │  юзер вводит https://hub.example.com → invoke("connect_hub")
      ▼  Rust: runtime CapabilityBuilder(remote origin){share-команды+core:event}
         → window.navigate(hub)
[веб-фронт хаба]  https://hub.example.com — существующий Next.js UI
      │  isTauri? getNativeBridge() → window.__MATEHUB_NATIVE__
      │  «Share screen» → нативный пайплайн вместо getDisplayMedia
      ▼
[нативный движок]  scap → OpenH264 → matehub-rtc-client → SFU :4001
```

- **Мост** (`bridge.js` ↔ `frontend/lib/native-bridge.ts`, version 1): узкий
  фасад `listShareTargets / startShare / stopShare / onShareStatus`. Веб-фронт
  не знает про Tauri IPC — только про этот интерфейс; в браузере
  `getNativeBridge()` возвращает `null`, и всё падает обратно на браузерный
  `publishScreen()`.
- **on_navigation**: webview заперт в origin активного хаба + локальные
  страницы; внешняя навигация блокируется (это клиент, не браузер).
- **Список хабов** (`hubs.rs`): dev-этап выбора; MRU в app config dir.
  Интеграция с SaaS-реестром (general/matehub.io) — когда у general появится
  desktop-API.

## Протокол нативного паблишера против SFU (ноль изменений в форвардере)

Клиент — обычный участник сессии, но со **своим participant-слотом**:
подключается тем же JWT с `?device=screen`, и SFU деривирует
`participant_id` из (session, user, device) — companion не выбивает основную
сессию юзера (веб/webview), сидящую в том же звонке. Auxiliary-соединение не
трогает hub-wide presence (occupancy/mute) — это состояние принадлежит
основной сессии. `device` — whitelist на сервере (`ALLOWED_DEVICES`), не
free-form: размножение participant'ов одним юзером — вектор злоупотребления.

Инварианты сняты с кода `services/video/src/sfu/mod.rs` и зафиксированы
loopback-тестами `crates/matehub-rtc-client/src/core_tests.rs` (ядро гоняется
в памяти против Rtc в конфигурации SFU: rtp_mode + full ICE):

1. **join** — оффер содержит ТОЛЬКО data channel: `MediaAdded` не стреляет,
   мёртвых треков нет, `publish_track`-хинт до join сервер дропает молча.
2. **publish_track** `{source:"screen", kind:"video"}` → **offer** с sendonly
   video m-line (строго в этом порядке — WS сохраняет порядок).
3. **Один слой, без simulcast** — v1-гейт SFU форвардит либо rid `h`, либо
   no-rid поток; слой `l` дропается на ингресте. Слать его — жечь аплинк.
   Simulcast вернётся со step-2 layer-select (Phase 3).
4. **PLI** приходит in-band (RTCP) как `Event::KeyframeRequest` →
   `PublisherEvent::KeyframeRequested` → `force_intra_frame()` энкодера.
5. **Glare**: серверный оффер, пока наш в полёте → отвечаем, воскрешаем
   изменения через `SdpApi::merge` (те же mid) и **переотправляем** оффер;
   застрявший в `queued_client_offer` старый SFU при дрейне отвергнет
   («Changed order for m-line»), повтор это перекрывает.
6. **Opus в codec-конфиге клиента**, даже пока аудио не шлём: SFU офферит
   audio m-line'ы чужих микрофонов, answer без единого PT невалиден.

Кодек — **H.264** (декодируют все зрители, включая Safari), пакетизация RFC
6184 — **str0m sample mode**: мы источник, а не релэй, поэтому баг
репакетизации (из-за которого SFU переехал на rtp_mode) нас не касается — на
всём пути ровно одна пакетизация.

## Медиа-стек

### Видео-энкодеры (`encoder/`)

Трейт `VideoEncoder` (encode BGRA→Annex B, force_keyframe, maybe_retarget)
прячет бэкенд; `encoder::select` пробует HW, при осечке — OpenH264:

| Бэкенд | Платформа | Крейт | Вход | Выход |
|---|---|---|---|---|
| VideoToolbox | macOS (сист. фреймворк) | `shiguredo_video_toolbox` | I420 | AVCC→Annex B |
| NVENC | Win/Linux+NVIDIA (feature `nvenc`) | `shiguredo_nvcodec` (dlopen) | I420 | Annex B |
| OpenH264 | все (fallback) | `openh264` | BGRA→I420 внутри | Annex B |

NVENC грузит CUDA через dlopen → собирается на GPU-less раннере, в рантайме
без NVIDIA `Encoder::new` падает → graceful fallback. Оба HW-энкодера pull/
callback приводятся к pull-контракту трейта через внутренний буфер (VT —
`next_frame`, NVENC — канал из callback). BGRA→I420 (`encoder/yuv.rs`) общий.

Настройки: High-профиль, длинный GOP, IDR по PLI, low-latency, ретаргет
битрейта по BWE (VT/NVENC — reconfigure, OpenH264 — пересоздание).

### Системный звук (`audio.rs`)

`scap captures_audio` → `Frame::Audio` (macOS: 48kHz stereo f32, **планарно
вопреки `is_planar()==false`** — интерливим) → нормализация к 48kHz stereo
(линейный ресемпл при ≠48k) → 20мс кадры → Opus → второй audio m-line
(source=screen). Уровень (dBov) стампуется для top-K SFU.

**Анти-фидбэк (свой звук не в захвате).** Системный аудио-тап тянет весь
микс, включая вывод НАШЕГО процесса (плеер нативного войса) → петля, звук
демонстрации «разъезжается». Лечится `excludesCurrentProcessAudio` (macOS
13+). upstream scap объявляет `exclude_current_process_audio`, но не
прокидывает его в ScreenCaptureKit — поэтому scap **вендорен**
(`apps/desktop-share/vendor/scap`, только src, без 4.9M баннера) с
однострочным патчем и подключён через `[patch.crates-io]`. Мы выставляем
флаг в `capture.rs`. **Windows/Linux:** scap там исключение процесса не
поддерживает (WASAPI process-loopback / PipeWire) — на этих платформах
петля возможна, пока не доработаем захват (или наушники/раздельные
устройства). На macOS (dev/приоритет) закрыто.

### Нативный войс (`voice.rs`)

Отдельное подключение (device=None — основной слот юзера) с микрофоном и
приёмом. Топология:

```
cpal in ─▶ ring ─▶ [mic] resample+20мс+Opus ─▶ SFU
SFU ─▶ RemoteAudio ─▶ [mixer] Opus decode per-mid + 20мс микс ─▶ ring ─▶ cpal out
```

- cpal-колбэки realtime → общение с рабочими потоками только через lock-free
  кольца (`ringbuf`); кодек в отдельных потоках.
- Микшер: per-mid Opus-декодер + playout-очередь (≤100мс), 20мс тик суммирует
  ≤3 говорящих (SFU top-K), хардклип.
- mute = не слать пакеты; deafen = тишина на выходе.
- **Координация с webview:** на десктопе webview в WebRTC-войс НЕ входит
  (`video-call-context` при наличии моста зовёт `joinVoice`/`leaveVoice` и
  зеркалит mute/deafen нативу вместо `vc.connect()`). Источник истины по
  медиа один — нативный клиент. В браузере мост отсутствует → прежний путь.

Почему нативно: WebRTC в webkit2gtk (Linux) сломан (wry не включает
enable-media-stream), в WKWebView — с оговорками. Нативный str0m-клиент даёт
одинаковый войс на всех трёх платформах.

## Сборка

Локально:

```bash
cd apps/desktop-share
npm ci                # @tauri-apps/cli
npx tauri dev         # dev-запуск (deep-link на macOS работает только из .app)
npx tauri build       # релизный бандл
```

Linux-зависимости — см. шаг «Install system dependencies» в
`.github/workflows/desktop-build.yml` (webkit2gtk 4.1, libpipewire-0.3-dev,
clang, nasm).

CI: пуш тега →

| Тег | Раннер | Артефакты |
|---|---|---|
| `desktop/mac-vX.Y.Z` | macos-latest (arm64 + кросс x86_64) | .dmg |
| `desktop/windows-vX.Y.Z` | windows-latest | .exe (NSIS), .msi |
| `desktop/linux-vX.Y.Z` | ubuntu-22.04 | .deb, .AppImage, .rpm |

Артефакты прикрепляются к GitHub Release тега + workflow artifacts (14 дней).
Сборки **не подписаны**: на macOS TCC-грант Screen Recording слетает при
пересборке (ad-hoc подпись меняется) — для внутренних сборок терпимо, для
дистрибуции нужны Developer ID + нотаризация (env-схема в комментарии
workflow).

## macOS: права на запись экрана (TCC) и подпись

Два подводных камня, оба уже частично закрыты в коде:

1. **Краш без прав (исправлено).** `scap::get_all_targets` (перечисление
   источников) делал `SCShareableContent::current().unwrap()` — без
   действующего разрешения на запись экрана это `Err` → `unwrap` роняет
   ВСЁ приложение (SIGABRT). Пропатчено в вендоренном scap (возврат пустого
   списка), плюс `capture::list_targets`/`pipeline` гейтятся на
   `has_permission()` и дают понятную ошибку вместо падения.

2. **«Права выданы, а приложение говорит что нет».** `has_permission()` =
   `CGPreflightScreenCaptureAccess` читает реальное состояние TCC, привязанное
   к **сигнатуре кода**. У неподписанной (ad-hoc) сборки сигнатура новая на
   каждый билд, поэтому:
   - грант для старого бинаря не применяется к новому (в Настройках висит
     запись по bundle id, но cdhash уже другой);
   - **macOS применяет грант только после полного перезапуска процесса** —
     после выдачи права надо Cmd-Q и открыть заново.

   **Обход для dev (заменили .app на новый):**
   ```bash
   tccutil reset ScreenCapture io.matehub.desktop-share
   # заново открыть приложение, выдать право в System Settings →
   # Privacy & Security → Screen Recording, затем ПОЛНОСТЬЮ выйти (Cmd-Q)
   # и открыть снова
   ```

   **Durable-фикс:** подписывать стабильной идентичностью. Для распространения —
   Developer ID + нотаризация (env-переменные в `desktop-build.yml`). Для
   локальной разработки — self-signed identity в keychain и
   `bundle.macOS.signingIdentity` в `tauri.conf.json`, чтобы cdhash (а значит
   и TCC-грант) не менялся между сборками.

## Известные ограничения / что дальше

- **Live-верификация медиа-стека не проведена**: захват+кодек+доставка в
  реальном звонке, качество войса (эхо, джиттер, дрейф часов), поведение на
  Windows/Linux с железом — требуют ручного/многостороннего прогона. Код
  компилируется на всех путях и покрыт юнит/loopback-тестами транспорта и DSP.
- **NVENC не запускался вживую** (нет NVIDIA у разработчика): написан против
  реального API крейта, собирается в CI; рантайм-путь на GPU не проверялся.
- **Приём удалённого ВИДЕО нативно не реализован** — видео-тайлы рисует
  webview-UI; нативный клиент берёт на себя только аудио (войс). H.264-декод
  + рендер — отдельный этап.
- **Джиттер-буфер войса упрощён** (playout-очередь + фиксированный 20мс тик);
  без PLC и подстройки под дрейф устройства — этого достаточно для v1, но на
  плохих сетях возможны артефакты. Софт-лимитер вместо хардклипа — позже.
- **Mic UI-состояние на десктопе** зеркалится в натив через эффект по
  `isMicEnabled`; полноценный ростер/индикаторы говорящих из нативных событий
  (`voice-status`) в UI пока не разведены.
- **Deep-link auth** (`matehub://`) не реализован — вход через веб-логин в
  webview (cookies персистентны).
- **Runtime capability нельзя отозвать** (Tauri 2.11): при смене хаба
  capability старого origin живёт до перезапуска. Митигировано точечным
  набором пермишенов + origin-guard в `start_share`/`join_voice`.
