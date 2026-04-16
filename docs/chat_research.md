# ТЗ: highload-сервис чата уровня Discord на Rust + TypeScript SDK

Документ описывает целевую архитектуру, модель данных, протокол, требования к подсистемам и план MVP для мультитенантного чат-сервиса с хабами (аналог guild) и текстовыми каналами. **Стек фиксируется: Rust-бэкенд, ScyllaDB для сообщений, PostgreSQL для метаданных, Elasticsearch для поиска, WebSocket + JSON/MessagePack как транспорт, TypeScript SDK.** Ключевой архитектурный приём — шардирование по `hub_id` уже в MVP с композитным partition key `(hub_id, channel_id, time_bucket)` по лекалам Discord, что позволяет линейно масштабировать хранилище и вынимать enterprise-хабы на dedicated-ноды без переписывания модели данных. Документ опирается на публичный опыт Discord (миграция Cassandra → ScyllaDB в 2022, 177 → 72 нод, p99 read 125 → 15 мс), MaxJourney-оптимизации для 16M-членных гильдий и на актуальную Rust-экосистему на начало 2026 (scylla 1.5, axum 0.8, fastwebsockets, NATS JetStream).

## 1. Целевая архитектура и её компоненты

Сервис строится как **несколько независимо масштабируемых слоёв**, разделённых по типу нагрузки: stateless gateway-слой с WebSocket, stateful слой персистентности, пуллы инфраструктуры (брокер, Redis, поиск). Граница доверия проходит на gateway — любой входящий фрейм проходит авторизацию, rate-limit и схемную валидацию до попадания во внутренний pub/sub.

**Gateway-ноды (Rust, Tokio).** Терминируют WebSocket, держат per-connection state (session_id, last_seq, подписки на каналы, presence), отвечают за heartbeat, RESUME, dispatch исходящих событий. Целевой sizing одной ноды — **100–500k idle-соединений** на 16-ядерной машине с 32–64 GiB RAM, что достижимо за счёт TLS через rustls, axum 0.8 для HTTP/health/metrics и fastwebsockets для горячего пути фреймов. Используется `SO_REUSEPORT` и Tokio current-thread runtime per core, чтобы избежать межъядерного contention в work-stealing scheduler при экстремальной конкурентности.

**API-ноды (Rust, Tokio).** Обслуживают REST/gRPC: отправка сообщений через HTTP (write-path с идемпотентностью), REST `/sync` для оффлайн-догона, CRUD метаданных хабов/каналов, модерационные операции, поиск. API-ноды stateless, общаются с тем же набором хранилищ, что и gateway.

**Data Service (Rust, Tokio, gRPC через tonic).** **Обязательный слой между бизнес-логикой и ScyllaDB** — ключевой урок Discord. Он инкапсулирует token-aware routing, request coalescing (одинаковые параллельные запросы к одной партиции сливаются в один поход в БД), кеш prepared-statements, ретраи и speculative execution. Именно он защищает кластер от горячих партиций и thundering herd при скачках нагрузки на популярный канал.

**Pub/sub-слой для fan-out между нодами.** **NATS JetStream** для горячего пути (sub-ms публикация, replay-окно 24–72ч) + **Kafka/Redpanda** как durable event log на 7–30 дней для audit и cross-region. Разделение ролей принципиально: NATS — latency-critical доставка между gateway-нодами, Kafka — долгосрочная персистентность событий и реплей при длинных дисконнектах.

**Redis (через `fred` 10.x).** Session registry (user_id → gateway_node_id с consistent hashing), presence с коротким TTL (30–90с), rate-limit buckets, счётчики непрочитанных, idempotency-кэш для ACK от клиента. Используется кластер Redis/Valkey.

**ScyllaDB.** История сообщений, hot path. Шардирование по `hub_id`, token-aware + shard-aware routing драйвером `scylla` 1.5. Отдельная таблица `messages`, отдельные для `reactions`, `message_edits`, `threads`.

**PostgreSQL (через sqlx 0.8).** Метаданные: users, hubs, channels, roles, permissions, webhooks, device-tokens. Row-level security по `hub_id` уже реализована в общем сервисе — интегрируемся через эту границу.

**Elasticsearch / альтернатива.** Полнотекстовый поиск по сообщениям. `elasticsearch-rs` до сих пор в alpha; на 2026 **более разумные варианты — Meilisearch (простая установка, отличная typo-tolerance) или Quickwit (S3-backed, подходит под чат-архив)**. Если Elasticsearch уже в компании — оставить его, но заложить адаптерный слой.

## 2. Модель данных и шардирование по `hub_id`

### 2.1 Снежинки (Snowflake IDs)

Все идентификаторы сущностей — **64-битные Snowflake** по лекалам Discord: 42 бита timestamp (мс от собственной эпохи), 10 бит worker/process, 12 бит sequence (4096 ID/мс/процесс). **Три принципиальных выигрыша**: ID сам по себе хронологически сортируем (отпадает отдельная колонка timestamp), ID можно генерить в любом процессе без координации, bucket для партиционирования вычисляется из ID чистой арифметикой без похода в БД. Используется `sonyflake` 0.2 (39 бит timestamp, 174 года жизни) или самописный генератор. Клиентские ID сообщений — UUIDv7 (RFC 9562), чтобы не раздавать worker-ID браузерам.

### 2.2 Схема ScyllaDB — сообщения

```cql
CREATE TABLE messages (
    hub_id         bigint,
    channel_id     bigint,
    bucket         int,            -- derived from message_id timestamp
    message_id     bigint,         -- snowflake
    author_id      bigint,
    content        text,
    thread_root_id bigint,
    mentions       frozen<set<bigint>>,
    mention_roles  frozen<set<bigint>>,
    mention_everyone boolean,
    attachments    frozen<list<text>>,
    edited_at      timestamp,
    deleted_at     timestamp,
    PRIMARY KEY ((hub_id, channel_id, bucket), message_id)
) WITH CLUSTERING ORDER BY (message_id DESC);
```

**Partition key — композитный `(hub_id, channel_id, bucket)`.** Это реализует шардирование по tenant_id уже на уровне модели: все данные одного хаба гарантированно остаются колокализованными (и могут быть вынесены на dedicated-кластер), а bucket защищает от разрастания партиции. Discord выбрали `bucket ≈ 10 дней`; **для нашего сервиса разумный старт — 7 дней**, с мониторингом `system.large_partitions` и автоматическим уменьшением bucket для каналов, приближающихся к 100 МБ. `message_id DESC` — самый частый запрос «последние N сообщений в канале» становится дешёвым сиквеншеным чтением из головы партиции.

**Рекомендуемые лимиты:** партиция **до 100 МБ** (жёсткое предупреждение ScyllaDB), типовая вставка до 1 МБ, одиночный запрос до 16 МБ. Превышение не ломает работу, но вызывает GC-давление при компакшене и неравномерное заполнение кэша.

### 2.3 Пустые bucket и надгробия

Implementation-gotcha от Discord: при массовом удалении в канале сканы надгробий (tombstones) по пустым bucket убивают производительность (инцидент Puzzles & Dragons). Решение: **поддерживать таблицу непустых bucket** `channel_buckets(hub_id, channel_id, bucket)` и итерироваться по ней, а не перебирать интервал. `gc_grace_seconds` снижаем с дефолтных 10 дней до **2 дней**, парируя этим nightly repair.

### 2.4 Сопутствующие таблицы

```cql
CREATE TABLE reactions (
    hub_id     bigint, channel_id bigint, message_id bigint,
    emoji      text,   user_id    bigint,
    created_at timestamp,
    PRIMARY KEY ((hub_id, channel_id, message_id), emoji, user_id)
);

CREATE TABLE message_edits (
    hub_id bigint, channel_id bigint, message_id bigint,
    edit_seq int, prior_content text, edited_at timestamp, editor_id bigint,
    PRIMARY KEY ((hub_id, channel_id, message_id), edit_seq)
);

CREATE TABLE read_state (
    user_id bigint, hub_id bigint, channel_id bigint,
    last_read_message_id bigint, mention_count int, updated_at timestamp,
    PRIMARY KEY ((user_id), hub_id, channel_id)
);

CREATE TABLE audit_log (
    hub_id bigint, day int, ts timestamp,
    actor_id bigint, action text, target_id bigint, diff text,
    PRIMARY KEY ((hub_id, day), ts)
);
```

Audit специально живёт **в отдельной партиции и с отдельным retention (180 дней)**, чтобы compliance-запросы не сканировали горячие партиции сообщений.

### 2.5 Token-aware и shard-aware routing

`scylla-rust-driver` 1.5 автоматически маршрутизирует prepared-statement на ноду-владельца токена **и на конкретное ядро (shard)**, минуя coordinator-hop. Требование: все запросы хот-пасса — только prepared, кешируемые через `CachingSession`. Для tenant-миграций драйвер поддерживает tablet-aware routing (ScyllaDB-specific).

### 2.6 Dedicated-ноды для enterprise

Поскольку partition key всегда начинается с `hub_id`, перенос крупного клиента на выделенный ScyllaDB-кластер сводится к **скоуп-копированию токен-ренджей этого hub_id**. Тот же Data Service разрешает `hub_id → cluster_endpoint` через конфиг/сервис-дискавери. Для самого gateway dedicated-нода — просто отдельный NATS subject-пространство и отдельная affinity-группа в load-balancer.

## 3. Протокол WebSocket-gateway

Протокол моделируется по opcode-лекалам Discord, но адаптируется под наш стек. Wire-формат — **JSON для браузеров, MessagePack (`rmp-serde`) для мобильных/нативных** клиентов; выбор через Sec-WebSocket-Protocol. `permessage-deflate` включается на браузерной стороне (сжатие 60–75% на тексте), на серверной — настраиваемо и выключено на максимально загруженных нодах из-за memory-фрагментации.

### 3.1 Opcode lifecycle

| Op | Направление | Назначение |
|----|------|---------|
| 10 HELLO | server→client | сразу после upgrade: `heartbeat_interval`, `resume_gateway_url` |
| 2 IDENTIFY | client→server | token, intents, device info |
| 0 DISPATCH | server→client | `{s, t, d}` — sequence, event type, payload |
| 1 HEARTBEAT | client→server | `{d: last_seq}` с jitter на первой отправке |
| 11 HEARTBEAT_ACK | server→client | подтверждение |
| 6 RESUME | client→server | `{session_id, seq}` при переподключении |
| 7 RECONNECT | server→client | просьба о graceful reconnect (deploys) |
| 9 INVALID_SESSION | server→client | `d: bool` — можно ли RESUME |

**RESUME — единственный путь догнать пропущенное при коротких дисконнектах.** Gateway буферизует исходящие события сессии в памяти (cap ~2 МБ, ~5 минут активности) и параллельно пишет в NATS JetStream-стрим `gateway.session.{session_id}` с retention 24–72ч. Если клиент возвращается в пределах буфера — replay напрямую из памяти; если позже, но в пределах stream retention — replay из JetStream; если за пределами — ответ `INVALID_SESSION(d=false)` и клиент должен повторно IDENTIFY и вызвать REST `/sync`.

### 3.2 Sequence numbers и гарантии доставки

**Модель at-least-once с клиентским dedup.** Каждое DISPATCH получает монотонный `s` в пределах сессии. Клиент в HEARTBEAT шлёт `last_seq` как ACK. При RESUMED, если `next_event.s > last_seq + 1` — gap, клиент обязан выполнить полный re-sync затронутых каналов через REST `/sync?since=...`.

Дубликаты на клиенте устраняются по `message_id` (Snowflake, глобально уникален). Для writes-path — idempotency через `Idempotency-Key: {client_message_id}` (UUIDv7 от клиента), сервер кеширует результат в Redis (`SETNX` TTL 24ч) и в PostgreSQL `UNIQUE(sender_id, client_id)` как последняя линия.

### 3.3 Heartbeat и зомби-соединения

Server шлёт `heartbeat_interval` ≈ 41с, клиент первую отправку сдвигает на `interval * random()` — классический anti-thundering-herd от Discord. Пропуск `HEARTBEAT_ACK` в течение `interval` = зомби: клиент **обязан закрыть с 4000 и пойти на RESUME**, не дожидаясь TCP-таймаута. На серверной стороне зомби выкидываются по тому же таймеру.

### 3.4 Шардирование gateway-соединений

Для боёв с миллионами соединений пользовательские сессии равномерно распределены по gateway-нодам через consistent-hashing по `user_id`. Это позволяет аффинность: все устройства одного пользователя попадают на одну ноду, что упрощает мультидевайс-синхронизацию и presence. Реестр `user_id → node_id` — в Redis с TTL. При потере ноды consistent-hash переназначает её долю на соседей, клиенты перезапускают IDENTIFY и RESUME не проходит (session_id привязан к ноде).

## 4. Fan-out сообщений и масштабирование под большие хабы

### 4.1 Базовая схема hybrid persist-once + publish-once + filter

**Write path сообщения:**
1. API-нода принимает HTTP POST, валидирует, вычисляет bucket и Snowflake message_id.
2. Пишет в ScyllaDB через Data Service (token-aware).
3. Публикует событие в NATS subject `hub.{hub_id}.channel.{channel_id}`.
4. Асинхронный ingest записывает событие в Kafka-топик (durable log, шардирование по `hub_id`) и индексирует в Elasticsearch/Meilisearch.
5. Gateway-ноды подписаны на subject-паттерны релевантных каналов; каждая нода **локально фильтрует** по подписчикам и пушит во WebSocket.

Ключевой приём — `@user`-меншены и push-нотификации разрешаются **не** через fan-out, а в отдельном notification-сервисе, который подписан на тот же NATS-топик и имеет доступ к `read_state`, device-tokens и user preferences.

### 4.2 Оптимизации для крупных хабов (урок MaxJourney)

Наивный fan-out в хабе на 100k+ участников — квадратичная работа. Применяем три оптимизации из публичного опыта Discord:

- **Lazy channel subscriptions.** Клиент не получает членский список целиком, а запрашивает «окно» `[0..99]` отсортированного списка участников и получает только дельты по этому окну. Реализуется через отдельный opcode (по аналогии с Discord OP 14) и отдельную таблицу-индекс в Redis.
- **Passive sessions.** Если пользователь не смотрит хаб (UI ушёл в background или свернул), gateway помечает сессию passive для этого хаба: не шлёт presence и typing, только новые сообщения в каналах с явной подпиской. На больших хабах это отсекает до 90% исходящего трафика.
- **Relay-шарды.** Для хабов с >15k одновременно онлайн-участников gateway-ноды группируются в relay-процессы; один «guild-процесс» публикует раз на relay, а relay фанаутит на свой пул сессий. В Rust-реализации это естественно ложится на пары `tokio::sync::broadcast` + `scc::HashMap<relay_id, subscribers>`.

### 4.3 Rate-limiting fan-out'а дешёвых событий

Typing-индикаторы и presence-дельты — **high-frequency ephemeral**, не персистируются никуда, не имеют sequence number (они не RESUM-ятся), fan-out только on-line и с троттлингом: не чаще 1 события / 5с на пользователя / канал. Отдельные NATS subject'ы `hub.{id}.typing.*` без JetStream.

## 5. Presence, typing, unread

Presence хранится **только в RAM gateway-нод + Redis TTL 90с**. На старте WebSocket gateway записывает `presence:{user_id}` в Redis через `fred`, heartbeat продлевает TTL, закрытие соединения удаляет ключ. Агрегаты («сколько онлайн в хабе») — через Redis HyperLogLog или приближённый счётчик `approximate_presence_count`, обновляемый раз в 30с.

**Statuses:** online / idle / dnd / invisible / offline. Статус публикуется как отдельный NATS-subject, подписчики — только активные (non-passive) сессии, смотрящие участника.

**Typing** — best-effort, TTL 10с, fan-out только в пределах текущего канала, никакого JetStream.

**Unread и mentions.** Счётчики per `(user_id, channel_id)` в `read_state` (ScyllaDB) + горячий кэш в Redis. Инкремент при fan-out, декремент/сброс по клиентскому `MESSAGE_READ` ивенту.

## 5.1 Вложения (Attachments)

### Upload flow

Вложение загружается **до** отправки сообщения (паттерн Discord/Slack):

1. Клиент выбирает файл, показывает preview/прогрессбар.
2. `POST /v1/channels/{channel_id}/attachments` (multipart) — chat service:
   - Авторизация: JWT, проверка permissions (WRITE на канал через hub service).
   - Валидация: тип файла (whitelist MIME), размер (free: 25 МБ, pro: 100 МБ).
   - Генерация ключа: `{hub_id}/{channel_id}/{snowflake_id}_{rand16}.{ext}`.
   - Upload в S3 бакет `matehub-chat-media` (авторизация на чтение, не публичный).
   - Ответ: `{attachment_id, url, type, name, size, width?, height?}`.
3. Клиент отправляет сообщение с `attachments: ["attachment_id_1", ...]`.
4. Сервер валидирует: attachment_id принадлежит этому юзеру, ещё не привязан к сообщению, не старше 24ч.
5. Сообщение сохраняется в ScyllaDB с `attachments: [{id, url, type, name, size}]`.

### Storage

- **Бакет:** `matehub-chat-media` (S3-совместимый, Yandex Object Storage / MinIO).
- **Доступ:** чтение с авторизацией (не публичный). Отдача через presigned GET URL с TTL 1ч, генерируемый chat service при запросе истории.
- **Структура ключей:** `{hub_id}/{channel_id}/{snowflake_id}_{random16hex}.{ext}`. Random suffix — защита от IDOR.
- **Quota:** per-hub, tracked в PostgreSQL metadata. Лимит free tier: 5 ГБ, pro: 50 ГБ.

### Типы файлов

| Категория | MIME types | Лимит |
|---|---|---|
| Изображения | image/jpeg, image/png, image/gif, image/webp | 25 МБ (free), 100 МБ (pro) |
| Видео | video/mp4, video/webm | 25 МБ / 100 МБ |
| Аудио | audio/mpeg, audio/ogg, audio/wav | 25 МБ / 100 МБ |
| Документы | application/pdf, text/plain, application/zip | 25 МБ / 100 МБ |
| Запрещено | application/x-executable, .exe, .bat, .sh, .ps1 | Reject |

### Thumbnail generation

- **Изображения:** сервер генерирует thumbnail 200x200 при upload (async worker или inline через `image` crate). Thumbnail сохраняется рядом: `{key}_thumb.webp`.
- **Видео:** ffmpeg thumbnail первого кадра (async worker). Phase 2.
- **Другие типы:** generic file icon на клиенте, без server-side preview.

### Content moderation

- **CSAM:** PhotoDNA hashing / Cloudflare CSAM scanning tool при upload. **Обязательно до публикации.** Блокировка + audit log + report.
- **Malware:** ClamAV scan на документах (async, не блокирует upload). Карантин до результата.
- **Explicit content:** NSFW detection (Phase 3). Spoiler-blur на клиенте.

### Orphan cleanup

- Вложения не привязанные к сообщению в течение 24ч — удаляются cron-job.
- `attachment_status` в Redis: `pending` (uploaded, not yet in message), `attached` (in message), `orphan` (expired).

### CDN (Phase 3)

При росте: CloudFront/Cloudflare перед S3 для кэширования. Presigned URL заменяется на signed cookie + CDN URL. Снижает нагрузку на S3 и chat service.

### Inline preview в сообщении

Клиент при рендеринге сообщения с attachments:
- Изображения: `<img>` с lazy loading, lightbox при клике.
- Видео: `<video>` с poster (thumbnail).
- Аудио: inline player.
- Документы: иконка + имя + размер + кнопка download.

## 5.2 Mentions — расширенное описание

### Парсинг

Парсинг выполняется **на сервере при ingestion**, не на клиенте. Клиент отправляет raw text с `@username`, `@groupname`, `@everyone`. Сервер:

1. Извлекает `@username` паттерны из content.
2. Резолвит username → user_id через PostgreSQL (кэш в Redis, TTL 5мин).
3. Резолвит `@groupname` → group_id через hub service.
4. Denormalized записывает в ScyllaDB поля `mentions`, `mention_groups`, `mention_everyone`.

### Уведомления

- Notification-сервис подписан на NATS `hub.*.channel.*.message`.
- Для каждого mention: проверяет user preferences (muted channel? DND?), push через APNs/FCM.
- `@everyone` / `@here` — fan-out по всем online (here) или all members (everyone) канала. Rate limited: не чаще 1 @everyone в минуту на канал.

### Unread mentions

- `read_state` в ScyllaDB: `(user_id, hub_id, channel_id) → {last_read_message_id, mention_count}`.
- При fan-out нового сообщения: если message mentions user → инкремент `mention_count` в read_state.
- При `MESSAGE_READ` от клиента: reset `mention_count = 0`, update `last_read_message_id`.
- Hot cache в Redis: `unread:{user_id}:{channel_id}` → `{count, mention_count}`.

### Клиентский рендеринг

- `@username` → `<span class="mention">@Username</span>` (кликабельный, показывает user card).
- `@groupname` → `<span class="mention role" style="color: {group.color}">@GroupName</span>`.
- `@everyone` → подсвечивается, визуально отличается.
- Autocomplete: при вводе `@` показывается dropdown с фильтрацией по member list.

## 6. Редактирование, удаление, реакции, треды, меншены

**Edit.** UPDATE в `messages` по `(hub_id, channel_id, bucket, message_id)` + append в `message_edits`. Событие `MESSAGE_UPDATE` через gateway. Клип истории на 50 последних версий.

**Soft delete** (по умолчанию): `deleted_at = now()`, контент обнуляется после 7-дневного grace-периода. Видимость — фильтр на уровне Data Service.

**Hard delete** (GDPR / CSAM): физическое удаление + event `MESSAGE_DELETE_HARD` в gateway, чтобы клиенты очистили IndexedDB-кэш. Удаление также из Elasticsearch (очередь async-purge).

**Reactions.** Отдельная таблица, partition key — та же, что у сообщения, clustering key `(emoji, user_id)`. Лимит 20 уникальных emoji на сообщение. События `REACTION_ADD/REMOVE`.

**Threads.** Поле `thread_root_id` в `messages`; треды — это просто «узкие» виртуальные каналы с тем же партиционированием. Отдельная таблица `threads(hub_id, parent_channel_id, thread_id, ...)` для метаданных.

**Mentions.** Парсинг на сервере при ingestion: `@user_id`, `@role_id`, `@everyone/@here`. Denormalized в `mentions`/`mention_roles`/`mention_everyone` поля `messages`. Разрешение ролей — кэшируется в Redis `role_members:{role_id}`. Notification-сервис подписан на события и делает per-recipient логику.

## 7. Протокол offline-синхронизации

Клиент располагает двумя механизмами догонки, выбираемыми по длительности оффлайна:

1. **WS RESUME** — короткие разрывы до ~5 минут (буфер в памяти) или до 72ч (JetStream retention). Replay сохраняет порядок, не дедуплицирует.
2. **REST `GET /sync?since={cursor}`** — при больших оффлайнах или gap detection. Возвращает per-channel `{events[], nextCursor, limited: bool}`. Поле `limited=true` = **слишком много событий, клиент обязан сбросить таймлайн канала** и подтянуть его фрешем по `/channels/:id/messages?before=...` (паттерн Matrix для corner-cases).

Пороги (Sendbird-like): если `server_seq - local_seq > 300` для канала, клиент делает full resync этого канала, не replay. Если оффлайн > retention event-лога → full resync хаба из текущего состояния (не replay-каждого-события).

## 8. End-to-End шифрование: решение и обоснование

**Целевая позиция: E2EE выключен по умолчанию для серверных хабов, опционально включается для приватных DM и enterprise secure rooms.**

Обоснование:

- **Discord, Slack, Telegram** не предоставляют E2EE для серверных сообщений ровно потому, что это ломает серверный full-text search, модерацию и Trust & Safety (включая обязательный CSAM-хеширование), ботов, полную историю для новых участников, серверную резолюцию меншенов для push. Для highload-чата эти возможности критичнее, чем криптографическая невидимость от провайдера.
- **Для 1:1 и малых DM-групп** применим Signal Double Ratchet либо MLS (RFC 9420). MLS — стандарт IETF июля 2023, спроектирован для групп до ~50k с O(log N) key-updates; уже в проде у Cisco Webex, Google RCS, Apple Messages. Rust-библиотека выбора — **openmls** 0.8+ (эталонная реализация, pluggable crypto через `openmls_libcrux_crypto`, pluggable storage).
- **Для enterprise secure rooms** — MLS с явным UX-предупреждением: поиск отключён (либо заменён локальным IndexedDB FTS5 по расшифрованному корпусу), боты недоступны, новым участникам история **не** восстанавливается (MLS Welcome раздаёт ключи только с текущего эпоха — принципиальное ограничение, а не баг).
- Сопутствующие потоки в E2EE-режиме также шифруются: реакции, typing, read-receipts (иначе протекает метаданные). `@mention` резолвится клиентом после расшифровки, push — silent-пуш с `{channel_id}`-only, клиент достаёт и расшифровывает.

Альтернатива Matrix Olm/Megolm в 2026 не рекомендуется: по формальному анализу (IACR 2023/1300 и arXiv 2408.12743) у Megolm слабее forward secrecy и нет криптографической аутентификации членства; `vodozemac` имел disclosure по MAC-truncation (февраль 2026), требующий явного V2-pinning. Ставим на **openmls** как на стандартный путь.

## 9. Rust-стек бэкенда (начало 2026)

Рекомендуемый фиксированный набор зависимостей:

```toml
[dependencies]
tokio              = { version = "1.47", features = ["full"] }
axum               = "0.8"
fastwebsockets     = { version = "0.10", features = ["upgrade"] }
tower              = "0.5"
tower-http         = { version = "0.6", features = ["trace","cors"] }
tower-governor     = "0.4"
scylla             = "1.5"
sqlx               = { version = "0.8", features = ["postgres","runtime-tokio-rustls","macros"] }
rdkafka            = { version = "0.37", features = ["tokio"] }
async-nats         = "0.38"
fred               = { version = "10", features = ["subscriber-client","i-all"] }
serde              = { version = "1", features = ["derive"] }
serde_json         = "1"
rmp-serde          = "1.3"
prost              = "0.14"
tonic              = "0.13"
tracing            = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter","json"] }
tracing-opentelemetry = "0.28"
opentelemetry-otlp = { version = "0.27", features = ["grpc-tonic"] }
metrics            = "0.24"
metrics-exporter-prometheus = "0.16"
dashmap            = "6"
scc                = "2"
governor           = "0.7"
sonyflake          = "0.2"
ts-rs              = "10"
```

**Принципиальные решения:**

- **axum 0.8 для маршрутизации + fastwebsockets для хот-пасса WebSocket**. axum::ws — это обёртка над tokio-tungstenite, что нормально для REST-эндпоинтов и низконагруженных каналов, но на 100k+ соединений fastwebsockets даёт 2–4× throughput. Используем `fastwebsockets::upgrade` интеграцию с hyper/axum.
- **scylla 1.5 обязательно через prepared + CachingSession**, иначе token-aware routing не работает (драйверу неоткуда взять partition-key metadata).
- **sqlx 0.8 для основных путей, tokio-postgres с pipelining для bulk-операций** (выгрузка истории, outbox-drainers). Различие в bulk-производительности реально 70–150× в экстремумах.
- **NATS JetStream (async-nats 0.38) для горячего fan-out + Kafka (rdkafka) для durable log.** Пытаться всё сделать на одном инструменте — плохой trade-off: Kafka слишком тяжёл для sub-ms inter-node fan-out, NATS недостаточно хорош для 30-дневного аудита и компактинга.
- **`fred` 10.x, а не `redis-rs`.** Ключевое различие — корректная работа с Redis Cluster, RESP3, auto-reconnect, keyspace-events.
- **Тип-шеринг с TypeScript — ts-rs 10.** Для плоских DTO это самое простое решение без runtime-overhead. Если появится мобильный нативный клиент (Swift/Kotlin) — мигрируем на `typeshare`. Specta оправдан только если строим типизированный RPC слой (rspc).

**Observability.** `tracing` + `tracing-opentelemetry` → OTLP Collector → Prometheus (через native OTLP receiver) + Tempo/Jaeger. Обязательные метрики: `ws_connections_active` (gauge), `ws_messages_in_total{hub,channel}`, `fanout_latency_ms` (histogram), `scylla_query_latency_ms`, `broker_publish_errors_total`, `session_resume_success_ratio`.

**Runtime.** Старт на Tokio multi-thread. Только если профилирование при >500k conn/node покажет scheduler contention — миграция на Tokio current-thread per core с `SO_REUSEPORT`. Переход на `glommio`/`monoio` (thread-per-core + io_uring) рассматривать только после чёткого обнаружения syscall-bottleneck, потому что ломает совместимость с половиной экосистемы.

**Rate limiting.** `governor` + `tower-governor` на уровне HTTP, собственная реализация на Redis counter для gateway-send limits. Модель Discord: per-route token buckets с ключом по top-level ресурсу (`hub_id`, `channel_id`, `user_id`), отдельный global limit ~50/s на токен, отдельный invalid-request budget (10k/10мин → Cloudflare ban). Ответы включают стандартные `X-RateLimit-*` заголовки.

## 10. TypeScript SDK

### 10.1 Публичный API и паттерны

**API-surface — типизированный EventEmitter как primary, async iterators для пагинации, без RxJS.** Индустрия (discord.js, matrix-js-sdk, stream-chat-js, Sendbird, Ably) единогласно на EventEmitter; RxJS добавляет ~40 КБ и навязывает парадигму консьюмеру. Используем `eventemitter3` 5.x с generic event-map:

```ts
type ChatEvents = {
  'message.new':      [msg: Message];
  'message.updated':  [msg: Message];
  'message.deleted':  [{ messageId: string; channelId: string }];
  'typing.start':     [{ userId: string; channelId: string }];
  'connection.state': [s: ConnectionState];
  'presence.update':  [{ userId: string; status: PresenceStatus }];
};
class ChatClient extends EventEmitter<ChatEvents> { /* ... */ }
```

Пагинация истории — async iterator: `for await (const page of channel.history({ pageSize: 50 }))`, естественно ложится на `useInfiniteQuery` из TanStack Query.

Структура namespace'ов: `client.hubs.get(id)`, `client.channels.get(id).messages.send(...)`, `client.channels.get(id).history(...)`, `client.users.me`, `client.presence.subscribe(userIds)`. Каждый sub-namespace экспортируется отдельным entry point для tree-shaking.

### 10.2 Reconnect, heartbeat, RESUME vs IDENTIFY

**Декоррелированный jitter** (AWS Architecture Blog), а не классический `2^n * base` — он порождает thundering herd при массовом восстановлении:

```ts
function nextDelay(prev: number, base = 1000, cap = 30_000) {
  return Math.min(cap, Math.floor(base + Math.random() * (prev * 3 - base)));
}
```

RESUME-ветка: при потере соединения с сохранёнными `{sessionId, lastSeq, resumeUrl, expiresAt}` — **reconnect на `resumeUrl` (не на initial URL), send RESUME**, до 3 попыток с backoff. На close-кодах auth/shard (4004/4010–4014) или после 3 подряд failed resumes — bail-out, fresh IDENTIFY. Важно: **reconnectAttempts сбрасывается только по READY/RESUMED, не по raw `ws.open`** (классический баг discord.js).

Browser edge-cases, которые **обязан** обработать SDK:

- `visibilitychange` → на `visible` сделать immediate ping, закрыть и переподключиться если нет ACK за 5с.
- `navigator.onLine`/`offline` → немедленный retry, не ждать таймер.
- Page Lifecycle API `freeze`/`resume` → проактивно закрыть WS (iOS Safari замораживает сокеты).
- Heartbeat app-level: native WebSocket API в браузере не даёт ping-фреймов, только JSON-сообщения.

### 10.3 Локальный кэш и offline sync

**Dexie 4.x** как IndexedDB-обёртка — industry-стандарт (WhatsApp Web, Microsoft To-Do, GitHub Desktop), ~22 КБ, live queries, нормальные миграции схемы. RxDB (~70 КБ) — overkill если свой протокол синхронизации. Схема:

```ts
db.version(1).stores({
  channels: 'id, hubId, lastMessageTs, lastReadTs',
  messages: '[channelId+ts], id, channelId, ts, senderId, clientId',
  outbox:   'clientId, channelId, createdAt, status',
  cursors:  'channelId',      // last_event_id per channel
  meta:     'key',             // sessionId etc.
});
```

Compound key `[channelId+ts]` делает query «последние 50 в канале» O(log N).

**Sync protocol:**
- Per-channel cursor `last_event_id` хранится в `cursors`.
- На reconnect — per active channel: POST `/sync {channelId, since}`, сервер отдаёт `{events, nextCursor, limited}`.
- `limited: true` → **сброс локального таймлайна канала**, backfill через пагинацию (паттерн Matrix, PR #3056).
- Gap detection внутри активной сессии: `event.seq !== lastSeq + 1` → backfill.
- Storage: `navigator.storage.estimate()`, `navigator.storage.persist()`. LRU-eviction при превышении quota; всегда сохраняется последние 100 сообщений на канал для instant-render.

### 10.4 Offline outbox, оптимистичный UI, идемпотентность

```ts
async function sendMessage(channelId: string, text: string) {
  const clientId = uuidv7();  // stable across retries = idempotency key
  const optimistic: Message = { id: clientId, clientId, channelId, text,
    status: 'sending', ts: Date.now(), senderId: me.id };
  store.addMessage(optimistic);  // renders instantly
  await db.outbox.put({ clientId, channelId, payload: {text},
    status: 'pending', attempts: 0, createdAt: Date.now() });
  return enqueueSend(clientId);  // background drainer
}
```

Drainer просыпается по `online`, по таймеру (1, 2, 4, ... cap 60с), по push-visibility. Шлёт HTTP с `Idempotency-Key: {clientId}`. На ACK — замена локальной строки `db.messages.put({...server, clientId})`, удаление из outbox. Terminal failure после N попыток → UI-кнопка «отправить ещё раз».

### 10.5 Рантайм-валидация и типизация

**Zod 3.x discriminated unions** для WS-фреймов — парсинг на trust-boundary обязателен. В проде — sampling 1% (parse стоит 5–20 мкс на фрейм, что при 1000 msg/s набегает). Типы backend↔SDK — через `ts-rs` как DTO (генерится `cargo test --features export-bindings`), commit в репо, CI-gate на расхождение.

### 10.6 React-интеграция

SDK — источник правды (хранит channels/messages/presence в своей памяти). Отдельный пакет `@chat/react`: хуки через `useSyncExternalStore`, live-queries Dexie для оффлайн-first, TanStack Query 5 + TanStack DB для paginated history с live-updates через `queryClient.setQueryData` при push-событиях. **Анти-паттерн: оборачивать push-события в `useQuery`**; push — это результат мутации, применяется через setQueryData.

### 10.7 Тестирование SDK

**MSW 2.7+** с `ws` namespace (появился в конце 2024) покрывает HTTP и WS одной библиотекой, Vitest в качестве runner, Playwright для E2E. Для низкоуровневых протокольных тестов (RESUME-flow, fake-timers на backoff) — `vitest-websocket-mock` поверх mock-socket. Test matrix: reconnect backoff, RESUME success + replay, RESUME fail → IDENTIFY fallback, gap detection, outbox retry on network flap, optimistic rollback on server NACK.

## 11. Phased план MVP

**Phase 0 — фундамент (шардинг сразу, без компромиссов).**
- ScyllaDB схема с `((hub_id, channel_id, bucket), message_id)` с первого коммита.
- Data Service (Rust + tonic) с token-aware routing, request coalescing, CachingSession.
- PostgreSQL интеграция через существующий сервис пользователей и RLS по `hub_id`.
- Snowflake-генератор в каждом сервисе.
- NATS JetStream-кластер (R=3).

**Phase 1 — gateway и базовый SDK.**
- axum + fastwebsockets gateway с opcode-протоколом (HELLO, IDENTIFY, DISPATCH, HEARTBEAT, RESUME).
- REST API: send message, edit, delete, get history, create channel, `/sync`.
- TypeScript SDK: EventEmitter-core, reconnect с декоррелированным jitter, RESUME/IDENTIFY state machine, Dexie-cache, outbox.
- Rate limiting (`governor` + Redis). Базовые метрики.

**Phase 2 — presence, typing, read state, меншены.**
- Redis presence с TTL, гонка ghost-sessions.
- Typing через NATS без JetStream.
- `read_state` таблица + unread-counters.
- Notification service с APNs/FCM и per-user muting/coalescing.

**Phase 3 — масштабные хабы и реакции.**
- Lazy channel-subscriptions (windowed member lists).
- Passive sessions в gateway.
- Relay-шарды для хабов >15k online.
- Reactions, threads.
- Soft/hard delete с retention policy.
- Audit log в отдельной партиции.

**Phase 4 — поиск, E2EE opt-in, dedicated enterprise.**
- Elasticsearch/Meilisearch индексация через Kafka consumer.
- MLS (openmls) для приватных DM и enterprise secure rooms.
- Tooling для миграции hub_id → dedicated ScyllaDB кластер.
- Multi-region с home-region pinning по guild.

## 12. Нефункциональные требования и целевые SLA

| Метрика | Целевой уровень | Ссылка на бенчмарк |
|---|---|---|
| p99 read message history | < 20 мс | Discord на ScyllaDB: 15 мс |
| p99 write message | < 15 мс | Discord: 5 мс |
| WS fan-out (persist → client receive) | p99 < 100 мс intra-region | — |
| Concurrent WS connections per gateway node | 100–500k | Rust + fastwebsockets sizing |
| Gateway message throughput (события/сек на ноду) | > 200k | — |
| RESUME success rate | > 99% для оффлайна < 5 мин, > 95% для < 24ч | — |
| Retention event log | 7 дней (JetStream) + 30 дней (Kafka audit) | — |
| Availability gateway | 99.95% | — |
| Availability write path | 99.9% | — |

## Выводы и ключевые архитектурные ставки

Проект стоит на **трёх нерушимых решениях, закладываемых в MVP**: композитный partition key `(hub_id, channel_id, bucket)` в ScyllaDB (обеспечивает одновременно мультитенантность, защиту от горячих партиций, путь к dedicated-нодам), обязательный Data Service-слой в Rust перед ScyllaDB (request coalescing — единственная реальная защита от thundering herd на горячих каналах, без которой Discord не смогли мигрировать), и разделение pub/sub-инфраструктуры на NATS JetStream (горячий fan-out, replay 72ч) + Kafka (durable log 30 дней). Попытка сэкономить на любом из трёх приведёт к дорогостоящему переписыванию через 6–12 месяцев по лекалам Discord 2022 года.

**E2EE — намеренно опциональный** и только для DM/enterprise rooms: для массового серверного чата потери (отсутствие серверного поиска, модерации, ботов, истории для новых участников) перевешивают гейн. Ставим на MLS (openmls) как стандарт, не на Megolm.

**TypeScript SDK** копирует проверенные паттерны discord.js/matrix-js-sdk/Sendbird: типизированный EventEmitter, Dexie для offline, UUIDv7 client-IDs + idempotency, декоррелированный jitter backoff, двухуровневый sync (RESUME + REST `/sync` с `limited`-семантикой). Ни один из этих элементов не является опциональным — все они закрывают конкретные баги, уже найденные в production других SDK.

Основной риск проекта — **не технологический, а операционный**: ScyllaDB требует понимания партиционирования и постоянного мониторинга `system.large_partitions`, NATS/Kafka-кластер требует compliance-планирования retention, а gateway-нода на 500k соединений чувствительна к sysctl-тюнингу, rustls-конфигурации и fd-лимитам. Рекомендуется с самого начала выделить dedicated DBRE/infra-инженера, а не ожидать, что application-команда вывезет это мимоходом.