# MateHub — Архитектурные решения

Документ фиксирует сквозные решения, не очевидные из чтения кода одного
сервиса: топологии развёртывания, SSO-флоу, single-WS-клиент, механику
прочитанных сообщений и streaming-proxy для вложений.

---

## 1. Топологии развёртывания

MateHub поставляется в двух режимах с одним и тем же бинарём `matehub-hub` и
сопутствующими сервисами (`matehub-chat`, `matehub-video`, `matehub-transcoder`).

```
┌────────────────────────────────────────────────────────────────────────┐
│                         SaaS control plane                              │
│                       (matehub.io — general)                            │
│   accounts · registry of hubs · mail · K8s provisioner · auth_codes     │
└───────────────────────────┬────────────────────────────────────────────┘
                            │  provisions + issues HUB_SECRET
                            ▼
┌────────────────────────────────────────────────────────────────────────┐
│              Hub-cluster (один на тенант, либо коробка)                 │
│                                                                         │
│   matehub-hub    ── Postgres: users, channels, groups, members, ...     │
│   matehub-chat   ── ScyllaDB: messages, read_state, attachments         │
│   matehub-video  ── SFU (str0m)                                         │
│   matehub-transcoder ── ffmpeg worker                                   │
│                                                                         │
│   JWT-секрет — HUB_SECRET, уникальный для этого хаба                    │
└────────────────────────────────────────────────────────────────────────┘
```

### Разделение ответственности

**general** — глобальный control plane. Существует только в SaaS-поставке,
никогда не выкатывается в закрытом контуре. Хранит:
- `accounts` — глобальные юзер-аккаунты (signup'ы через лендинг)
- `hubs` — реестр хабов (slug, creator, status, **hub_secret**)
- `hub_members` — ACL доступа «этот account может войти в этот hub»
- `auth_codes` — одноразовые коды SSO-handoff (см. п. 2)
- `mail_outbox` — отложенная отправка писем

**hub-cluster** — per-tenant. Один кластер обслуживает один хаб. Все данные
хаба (пользователи, каналы, сообщения, группы, права, вложения) локальны. До
general не достаёт никогда на runtime-пути.

### Два пространства ID

Snowflake-генерация разделена на непересекающиеся области:

| Пространство | Генерирует | Примеры |
|---|---|---|
| **general**-уровень | general (только в SaaS) | `accounts.id`, `general.hubs.id` |
| **hub**-уровень | сам hub-под | `hub.users.id`, `hub.channels.id`, `hub.groups.id`, `hub.temp_users.id`, `chat.messages.message_id`, `chat.attachments.attachment_id`, `hub.hubs.id` |

Id'ы не обязаны совпадать между пространствами. `general.hubs.id` и
`hub.hubs.id` могут быть равны по соглашению (при провижене general кладёт
свой id в ENV пода), но взаимосвязь не строгая — в on-prem hub генерит свой
id сам при первом старте.

Sonyflake использует `machine_id` — по умолчанию младшие 16 бит private IP
пода. В K8s — уникально по IP, в коробке — один pod = один machine_id. Между
разными deployment'ами столкновений быть не может (нет сетевой связности).

### Temp users

Отдельная таблица `hub.temp_users` — гости, приглашённые через magic-link.
В `general` их никогда нет. JWT для temp user имеет `user_type: "temp"`,
`sub: temp_users.id`. Флоу `/invite/{token}` полностью локальный для hub'а.

---

## 2. SSO-флоу (SaaS режим)

В SaaS-поставке юзер логинится один раз на `matehub.io` (general), оттуда
«входит» в конкретный хаб без повторного ввода credentials. Реализуется
обрезанным OAuth **authorization code grant** — мы владеем обеими сторонами,
полный OIDC с JWKS-discovery оверкилл.

### Ключевые решения

**У каждого хаба — свой JWT-секрет.** При провижене general генерирует
32-байтный случайный `HUB_SECRET`, хранит в `general.hubs.hub_secret`,
передаёт подовому деплойменту через K8s Secret → ENV `HUB_SECRET`. Hub
подписывает свои JWT этим секретом; токен хаба A невалиден в хабе B, даже
если оба в одном SaaS-инстансе. Компрометация одного hub'а не раскатывает
по остальным.

**Ни general, ни hub не выдают токены друг за друга.** General держит
session cookie только для своей панели (`matehub.io`), hub — для своей
(`slug.matehub.io`). Cookie scope **per-subdomain**, не `.matehub.io` —
иначе бы утекало между хабами. Точка передачи — только одноразовый
`auth_code` на 30 секунд, single-use.

**`HUB_SECRET` двойного назначения:** (а) подпись JWT этого hub'а,
(б) Bearer для server-to-server канала `hub → general /internal/auth/exchange`.

### Флоу

```
┌──────────┐              ┌─────────────┐              ┌───────────────┐
│ Browser  │              │ general     │              │ hub pod       │
│          │              │ matehub.io  │              │ slug.matehub.io│
└────┬─────┘              └──────┬──────┘              └───────┬───────┘
     │                           │                             │
     │ 1. POST /auth/login       │                             │
     │    (email+password)       │                             │
     ├──────────────────────────►│                             │
     │                           │                             │
     │ 2. Set-Cookie:            │                             │
     │    general_session        │                             │
     │    (scope: matehub.io)    │                             │
     │◄──────────────────────────┤                             │
     │                           │                             │
     │ 3. Click "Enter Hub X"    │                             │
     ├──────────────────────────►│                             │
     │                           │                             │
     │                           │ 4. Создать auth_code 30s    │
     │                           │    {code, account_id,       │
     │                           │     hub_id, expires_at}     │
     │                           │                             │
     │ 5. 302 Location:          │                             │
     │    slug.matehub.io        │                             │
     │    /auth/sso?code=...     │                             │
     │◄──────────────────────────┤                             │
     │                           │                             │
     │ 6. GET /auth/sso?code=... │                             │
     ├───────────────────────────┼────────────────────────────►│
     │                           │                             │
     │                           │ 7. POST /internal/auth/exchange
     │                           │    Bearer: HUB_SECRET       │
     │                           │    { code }                 │
     │                           │◄────────────────────────────┤
     │                           │                             │
     │                           │ 8. { account_id, email,     │
     │                           │      display_name }         │
     │                           │    + invalidate code        │
     │                           ├────────────────────────────►│
     │                           │                             │
     │                           │          9. Upsert hub.users│
     │                           │             (account_id UNIQUE)
     │                           │             Issue JWT       │
     │                           │             (подпись HUB_SECRET)
     │                           │                             │
     │ 10. Set-Cookie:           │                             │
     │     hub_session           │                             │
     │     (scope: slug...)      │                             │
     │     302 Location: /       │                             │
     │◄──────────────────────────┼─────────────────────────────┤
     │                           │                             │
     │ 11. Далее hub_session     │                             │
     │     против hub'а; general │                             │
     │     в пути не участвует   │                             │
     ├───────────────────────────┼────────────────────────────►│
```

### Маппинг identity

В хабе `users` имеет опциональную колонку `account_id BIGINT UNIQUE NULL`.
Заполнена только для SaaS-провиженных юзеров (первое SSO делает UPSERT). Для
on-prem и temp_users — `NULL`. Внутри hub'а вся логика оперирует
hub-локальным `users.id`; `account_id` — только точка встречи двух ID-
пространств при handoff.

### Коробочный режим

Endpoint `/auth/sso` не регистрируется. Вместо него — обычная форма
`/auth/login` (email+password по `hub.users.password_hash`), выдающая
`hub_session`. `HUB_SECRET` генерится hub'ом самостоятельно на первом старте
и сохраняется в локальный конфиг. General отсутствует в периметре — hub не
знает о его существовании и не пытается дозвониться.

### Temp users

Параллельный флоу `/invite/{token}` → `hub.temp_users` → JWT с
`user_type: "temp"`, `sub: temp_users.id`. Не пересекается ни с general, ни с
SSO-механикой.

---

## 3. Один WebSocket на пользователя (chat)

### Мотивация

В старой реализации `useChatClient(channelId)` создавал свой `ChatClient` на
каждый активный канал. Переключение канала в сайдбаре пересоздавало WS,
IDENTIFY-перепрокатывал, сессия терялась. NATS-подписка на стороне gateway
уже была `hub.{hub_id}.channel.>` — то есть сервер был готов слать события
всех каналов в один сокет, но клиент этим не пользовался.

### Решение: `ChatProvider`

Один `ChatClient` живёт на уровень `HubLayout` через
`frontend/contexts/chat-context.tsx`. Пересоздаётся только при смене
`session.token`/`session.hubId` (relogin/logout).

```
HubLayout
  └── ChatProvider          ← владеет ChatClient, WS, map'ами состояния
       └── SidebarProvider
            └── AppSidebar
                 └── NavChannels  ← useUnreadCounts() (селектор)
            └── ...
                 └── TextChannelView ← useChatClient(channelId) (селектор)
```

### Состояние

```ts
{
  client: ChatClient | null,
  connectionState: ConnectionState,
  messagesByChannel:  Map<number, ChatMessage[]>,   // cached per-channel history
  typingByChannel:    Map<number, string[]>,         // typing users per channel
  unreadByChannel:    Map<number, {lastRead, unread}>,
  dividerByChannel:   Map<number, number | null>,    // "New" divider snapshots
  activeChannelId:    number | null,
}
```

### `useChatClient(channelId)` — селектор

Хук сохраняет прежнюю сигнатуру (`messages`, `typingUsers`, `sendMessage`,
`sendTyping`, `loadMore`, `connectionState`) плюс новые `dividerPos` и
`retryMessage`. Внутри — `useChatContext()` + вычитывание нужных кусков
карт. Переключение канала меняет `activeChannelId`, **не** трогает WS.

### Lazy history

`ensureHistory(channelId)` вызывается хуком при первом монтировании. Делает
`GET /messages?limit=50`, кладёт результат в `messagesByChannel`.
Повторный вход в канал — мгновенный, без сетевого обращения.

### Побочные эффекты

- Session_id на сервере живёт до logout → `seq` счётчик растёт →
  gateway RESUME идеально покрывает кратковременные дисконнекты (wifi
  flapping, suspend laptop).
- NATS-подписка одна на клиента вместо одной на канал → нагрузка на
  gateway-pod ≈ `len(active_users)`, а не `len(active_users × channels)`.

### Звуки

`message-out` играется всегда при моём отправленном сообщении
(подтверждение). `message-in` — только когда `activeChannelId === msg.channel_id`,
чтобы не было ding-spam от фоновых каналов.

---

## 4. Прочитанные/непрочитанные

### Источники истины

- `matehub_chat.read_state` (Scylla) — per-user per-channel
  `last_read_message_id`. Источник истины.
- Redis — опциональный hot-cache unread-счётчиков + ephemeral idempotency/
  rate-limiting. Read_state туда **не** кешируется (на MVP — пересчитываем
  на лету).

### Bulk fetch при входе в хаб

`ChatProvider` на коннекте делает две HTTP-операции:

**(a)** `GET /v1/read-states` → `[{channel_id, last_read_message_id, mention_count}]`
для всех каналов, где у юзера есть read_state. Нужно для `lastRead`
снапшотов (основа для дивайдера) и для списка каналов, которые
бэкфиллить.

**(b)** `POST /v1/sync` батчами по 50 каналов, payload
`{channels: [{channel_id, after: last_read_message_id}]}`. Возвращает
сообщения за период от `lastRead+1` до текущего. Для каждого канала
фронт считает `count(messages.filter(author_id != myId))` — это точный
unread. Если `limited: true` (бэк обрезает на 300 событий) — ставим
`Math.max(300, count)`, сайдбар всё равно клампит отображение до `99+`.

Этот двойной round-trip даёт точные unread-бейджи сразу после логина, без
ожидания «открой канал → пересчитаю». Альтернатива — dedicated endpoint
`GET /v1/unread-counts` — была отброшена как дублирование уже существующего
`/sync` ради экономии кода.

### Forward-инкремент через WS

На каждое `MESSAGE_CREATE`:

```
myMessage?          → advance lastRead, unread=0, no server ACK
active channel?     → ackUpTo(channel, msg.id)  — POST /ack, lastRead=msg, unread=0
otherwise           → unread += 1, lastRead неизменно
```

### Авто-ACK при фокусе

При смене `activeChannelId` на `X`:
1. **Снапшот divider'а** `= unreadByChannel.get(X).unread > 0 ? lastRead : null`.
   Снимок обновляется при каждом заходе в канал (leave + re-enter даёт свежий
   divider для новых unread).
2. **ACK до newest loaded** — если история закеширована и newest.message_id
   > lastRead, дёргается `ackUpTo(X, newest.id)`.

Если история ещё грузится, второй шаг выполняется в `.then()` после
завершения `getHistory` — только если на этот момент активный канал всё ещё
`X` (юзер не успел уйти).

### Unread divider в чате

`<UnreadDivider>` рисуется перед первым сообщением с `message_id > dividerPos`.
Только перед первым — последующие сообщения после дивайдера рендерятся
обычным потоком. При arriving-message-while-active divider не двигается
(pin'ится к точке входа), новые сообщения просто падают ниже него.

### Race-окно при бэкфилле

Между моментом `sync`'овского Scylla-read'а и обработкой ответа на клиенте
могут прилететь живые `MESSAGE_CREATE` и инкрементнуть `unread`. При записи
sync-результата используется `Math.max(row.unread, syncCount)` — что бы ни
было больше. NATS публикуется **после** write в Scylla, так что:
- Если сообщение было persist'нуто до sync-read'а → попадёт в sync-результат;
  инкремент + replace дадут одинаковое значение, max = оно.
- Если сообщение было persist'нуто после sync-read'а → не попадёт в sync, но
  инкремент его учтёт → max сохранит инкремент.

Недостача макс в 1 теоретически возможна только в микросекундном зазоре
между persist и увидеть-в-read — пренебрежимо.

---

## 5. Stream attachment endpoint

`GET /v1/attachments/{attachment_id}/stream` — аутентифицированный
streaming-proxy между клиентом и S3 с поддержкой HTTP Range.

### Sidecar-индекс

Инлайн-хранилище аттачей в `messages.attachments` (list<text>) не позволяет
быстро найти аттач по id без сканирования messages. Заведена отдельная
таблица `matehub_chat.attachments`:

```sql
CREATE TABLE matehub_chat.attachments (
    attachment_id text PRIMARY KEY,
    hub_id        bigint,
    channel_id    bigint,
    message_id    bigint,
    bucket        int,
    url           text,
    content_type  text,
    size          bigint
)
```

Hub_id дублируется, чтобы stream-хендлер мог авторизовать без join.

### Когда пишется

- **`send_message`**, после `write_message`, один `INSERT` на каждый
  `Attachment` с непустым `id`. Почему не в `upload_attachment`: аттач до
  отправки сообщения — «мусор», который некому чистить. Привязываем индекс
  к моменту коммита в сообщение.
- **`transcode.result` (Ok)**: `UPDATE attachments SET url=?, content_type=?,
  size=?`. Исходный .mov-URL заменяется на .mp4 после завершения
  транскодирования — индекс остаётся актуальным.

### Чтение + прокси

```
1. Lookup по attachment_id в индексе.
   NotFound → 404.
2. row.hub_id vs auth.0.hub_id.
   Не совпадает → 403 (cross-hub access denied).
3. reqwest::Client::new().get(row.url).
   Если на клиентском запросе есть `Range:` — форвардим как есть.
4. S3 отдаёт 200 или 206 + body-stream.
   Status и выборочные headers (Content-Length, Content-Range,
   Last-Modified) пробрасываем клиенту.
   Content-Type берём из индекса, не из S3.
   Accept-Ranges: bytes — проставляем сами.
5. Body — через Body::from_stream(bytes_stream) — память ограничена
   reqwest-буфером, файл на 1 GB не ест 1 GB RAM.
```

### Почему прокси, а не 302→S3

- **Authn остаётся серверной.** Утечка attachment_id не даёт доступ без JWT
  в нужном хабе. При 302-редиректе на публичный S3-URL этот барьер исчез бы.
- **Endpoint/bucket layout скрыт** от клиента.
- **Миграция на приватный bucket** (presigned URLs) — это одна правка
  `req_builder` в хендлере, без апдейта клиентов.

### Что не сделано (намеренно, отложено)

- Фронт по-прежнему использует прямые S3-URL из `attachment.url` (работает
  с публичным bucket'ом). Переключение на `/stream` — отдельный тикет,
  когда будет закрытие bucket'а.
- Нет метрик/кеша на уровне proxy. При наличии CDN перед S3 (CloudFront и
  т.п.) большинство запросов не долетает до нас вообще. Если станет узким
  местом — Varnish/nginx sidecar.
