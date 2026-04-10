# MateHub -- Структура проекта

Monorepo с двумя workspace'ами: **Cargo** (Rust backend'ы) и **npm** (TypeScript frontend + SDK).

---

## Дерево

```
matehub/
├── Cargo.toml                 # Rust workspace root
├── package.json               # npm workspace root
├── .gitignore
│
├── docs/                      # Документация
│   ├── project-structure.md   # <- ты здесь
│   └── video-service-spec.md  # ТЗ видеосервиса
│
├── services/                  # Rust backend'ы (Cargo workspace members)
│   ├── video/                 # matehub-video   -- SFU, WebRTC (str0m)
│   │   ├── Cargo.toml
│   │   └── src/
│   ├── chat/                  # matehub-chat    -- чат-сервис
│   │   ├── Cargo.toml
│   │   └── src/
│   └── general/               # matehub-general -- профили, каналы, auth
│       ├── Cargo.toml
│       └── src/
│
├── frontend/                  # @matehub/frontend (Next.js 16 + shadcn)
│   ├── app/                   # Next.js App Router (pages, layouts)
│   ├── components/            # React-компоненты
│   │   └── ui/                # shadcn UI primitives (button, etc.)
│   ├── hooks/                 # React hooks
│   ├── lib/                   # Утилиты (cn, etc.)
│   └── package.json
│
└── packages/                  # TypeScript SDK'и (npm workspace packages)
    ├── sdk-video/             # @matehub/sdk-video -- WebRTC client library
    │   ├── src/
    │   │   ├── client.ts      # VideoClient class
    │   │   ├── types.ts       # VideoClientOptions, SessionInfo, etc.
    │   │   └── index.ts       # Public API exports
    │   └── package.json
    └── sdk-chat/              # @matehub/sdk-chat  -- Chat client library
        ├── src/
        │   ├── client.ts      # ChatClient class
        │   ├── types.ts       # ChatClientOptions, Message, etc.
        │   └── index.ts
        └── package.json
```

---

## Два workspace'а

### Cargo (Rust)

Корневой `Cargo.toml` определяет workspace с общими зависимостями. Все сервисы наследуют версии через `workspace = true` -- одно место для обновления версий.

```toml
# Cargo.toml (root)
[workspace]
members = ["services/video", "services/chat", "services/general"]

[workspace.dependencies]
tokio = { version = "1", features = ["full"] }
# ... остальные зависимости
```

```toml
# services/video/Cargo.toml
[dependencies]
tokio.workspace = true   # наследует версию из корня
str0m.workspace = true   # только у video -- WebRTC не нужен chat и general
```

### npm (TypeScript)

Корневой `package.json` определяет npm workspaces. SDK-пакеты доступны по имени без публикации:

```json
{
  "workspaces": ["frontend", "packages/*"]
}
```

Frontend импортирует SDK напрямую:
```ts
import { VideoClient } from "@matehub/sdk-video";
import { ChatClient } from "@matehub/sdk-chat";
```

---

## Команды

### Rust (из корня проекта)

```bash
# Проверка всего workspace
cargo check

# Собрать всё
cargo build

# Собрать конкретный сервис
cargo build -p matehub-video
cargo build -p matehub-chat
cargo build -p matehub-general

# Запустить конкретный сервис
cargo run -p matehub-video
cargo run -p matehub-chat
cargo run -p matehub-general

# Тесты
cargo test                    # все тесты
cargo test -p matehub-video   # тесты видеосервиса

# Release build
cargo build --release -p matehub-video
```

### Frontend (из корня проекта)

```bash
# Dev server (Next.js + Turbopack)
npm run dev:frontend

# Build
npm run build:frontend

# Lint
npm run lint:frontend

# Type check
npm run typecheck
```

Или из директории `frontend/`:

```bash
cd frontend
npm run dev       # http://localhost:3000
npm run build
npm run lint
npm run typecheck
```

### SDK пакеты

```bash
# Type check SDK
cd packages/sdk-video && npm run typecheck
cd packages/sdk-chat && npm run typecheck

# Build SDK (компилирует TS -> JS + .d.ts в dist/)
cd packages/sdk-video && npm run build
```

### shadcn (добавление UI-компонентов)

```bash
cd frontend
npx shadcn@latest add dialog
npx shadcn@latest add input
npx shadcn@latest add avatar
# компоненты появятся в frontend/components/ui/
```

---

## Сервисы -- кто за что отвечает

### matehub-general (services/general/)

Владеет: Hub'ами, пользователями, каналами (channels), permissions.

Hub -- верхнеуровневая сущность (аналог Discord Server). Пользователь создаёт Hub и получает пространство с каналами, участниками, настройками. Channel -- persistent-сущность внутри Hub. Voice channel существует от создания до удаления, к нему привязан чат. Когда пользователь нажимает "Join Voice", General Service запрашивает SFU Session у Video Service и возвращает клиенту connection info.

### matehub-video (services/video/)

Владеет: SFU Sessions, WebRTC media routing.

SFU Session -- transient. Создаётся когда первый участник заходит в канал, уничтожается когда последний вышел. Не знает о каналах, профилях, чатах -- только `channel_id` как внешний ключ.

Стек: str0m (WebRTC), tokio (signaling), dedicated threads (media plane).

Подробная спецификация: [video-service-spec.md](./video-service-spec.md)

### matehub-chat (services/chat/)

Владеет: сообщениями, историей чата, доставкой.

Подписывается на NATS-события от Video Service для системных сообщений ("User A joined voice", "Recording available").

### @matehub/sdk-video (packages/sdk-video/)

TypeScript-библиотека для подключения к Video Service из браузера (и потенциально React Native). Оборачивает browser WebRTC API + signaling WebSocket в удобный интерфейс:

```ts
const client = new VideoClient({ wsUrl, token, iceServers });
await client.connect();
await client.publishCamera();
await client.publishScreen();
client.on("trackAdded", (track) => { /* render remote video */ });
```

### @matehub/sdk-chat (packages/sdk-chat/)

TypeScript-библиотека для подключения к Chat Service. WebSocket-клиент с типизированными сообщениями:

```ts
const chat = new ChatClient({ wsUrl, token });
await chat.connect();
chat.sendMessage(channelId, "Hello");
chat.on("message", (msg) => { /* render in UI */ });
```

---

## Добавление нового сервиса

### Rust сервис

1. Создать директорию:
```bash
mkdir -p services/newservice/src
```

2. Создать `services/newservice/Cargo.toml`:
```toml
[package]
name = "matehub-newservice"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
tokio.workspace = true
axum.workspace = true
# ... нужные зависимости из workspace
```

3. Создать `services/newservice/src/main.rs`

4. Добавить в корневой `Cargo.toml`:
```toml
[workspace]
members = [
    "services/video",
    "services/chat",
    "services/general",
    "services/newservice",   # <-- добавить
]
```

5. Проверить: `cargo check -p matehub-newservice`

### TypeScript SDK пакет

1. Создать директорию:
```bash
mkdir -p packages/sdk-newfeature/src
```

2. Создать `packages/sdk-newfeature/package.json`:
```json
{
  "name": "@matehub/sdk-newfeature",
  "version": "0.0.1",
  "type": "module",
  "private": true,
  "main": "dist/index.js",
  "types": "dist/index.d.ts",
  "scripts": {
    "build": "tsc",
    "typecheck": "tsc --noEmit"
  },
  "devDependencies": {
    "typescript": "^5.9.3"
  }
}
```

3. npm workspace подхватит автоматически (паттерн `packages/*` в корневом `package.json`).

4. `npm install` из корня для линковки.

---

## Shared-зависимости (Rust)

Все версии зависимостей определены в корневом `Cargo.toml` в секции `[workspace.dependencies]`. Сервисы подключают их через `dependency.workspace = true`.

Если зависимость нужна **только одному сервису** (например, str0m только для video), она всё равно объявляется в workspace dependencies для единообразия, но подключается только в Cargo.toml того сервиса, которому нужна.

Обновление версии зависимости -- одно изменение в корневом `Cargo.toml`, и все сервисы получают новую версию.

---

## Shared Rust crate (при необходимости)

Если между сервисами появится общий код (protobuf-определения, общие типы, middleware), создать shared crate:

```
services/
├── shared/              # matehub-shared
│   ├── Cargo.toml
│   └── src/lib.rs
├── video/
├── chat/
└── general/
```

И подключить как зависимость:
```toml
# services/video/Cargo.toml
[dependencies]
matehub-shared = { path = "../shared" }
```

Не создавать заранее -- только когда реально появится дублирование.

---

## Infrastructure & Deploy

```
matehub/
├── deploy/
│   ├── docker-compose.dev.yml        # Local dev infra (Redis, NATS, PG, MinIO, TURN)
│   └── docker/
│       ├── Dockerfile.video           # Rust multi-stage, debian:bookworm-slim (needs libssl)
│       ├── Dockerfile.chat
│       ├── Dockerfile.general
│       └── Dockerfile.frontend        # Next.js standalone
│
└── scripts/
    ├── dev-up.sh                      # Start local infra
    └── dev-down.sh                    # Stop local infra
```

### Local development workflow

```bash
# 1. Start infrastructure
./scripts/dev-up.sh
#    Redis:    localhost:6379
#    NATS:     localhost:4222
#    Postgres: localhost:5432
#    MinIO:    localhost:9000
#    TURN:     localhost:3478

# 2. Run services (each in separate terminal)
cargo run -p matehub-general
cargo run -p matehub-video
cargo run -p matehub-chat
npm run dev:frontend       # http://localhost:3000

# 3. Stop infrastructure
./scripts/dev-down.sh
```

### Docker build

```bash
# Build from repo root (context = repo root for Cargo workspace access)
docker build -f deploy/docker/Dockerfile.video -t matehub-video .
docker build -f deploy/docker/Dockerfile.chat -t matehub-chat .
docker build -f deploy/docker/Dockerfile.general -t matehub-general .
docker build -f deploy/docker/Dockerfile.frontend -t matehub-frontend .
```

Rust Dockerfiles используют multi-stage build с кешированием зависимостей:
первый слой компилирует только Cargo.toml (dependencies), второй -- source.
Это значит что rebuild после изменения кода = ~30 секунд, а не 5 минут.

**Важно:** Rust-сервисы собираются с `debian:bookworm-slim` (не Alpine), потому что
str0m зависит от OpenSSL (libssl). Alpine использует musl libc, и сборка OpenSSL
там требует дополнительной возни.
