# MateHub — коробочная установка

MateHub целиком на одном Linux-сервере. Все компоненты (Postgres,
ScyllaDB, Redis, NATS, MinIO, TURN, SPA, четыре Rust-сервиса, Caddy)
живут одним `docker compose` стеком за одним доменом. Никаких SaaS,
Kubernetes или внешних зависимостей кроме самой ОС.

Архив самодостаточен. Три скрипта:

| | |
|---|---|
| `setup.sh` | Ставит Docker, генерирует `.env`, спрашивает про домен / режим S3 / observability. Запускается **один раз** на хост. |
| `up.sh`    | Тянет образы, поднимает стек, печатает адреса и креды. |
| `down.sh`  | Останавливает стек. `--purge` также чистит данные (заводской сброс). |

---

## Минимальные требования

| Ресурс | Без observability | С observability (Grafana + ELK) |
|---|---|---|
| **ОС** | Linux: Debian 12+ / Ubuntu 22.04+ / RHEL 9 / AlmaLinux 9 / Rocky 9 / Fedora | то же |
| **CPU** | 2 vCPU | 4 vCPU |
| **RAM** | 4 GB | 8 GB |
| **Диск** | 30 GB SSD | 50 GB SSD |
| **Сеть** | 1 публичный IPv4, порты ниже | то же |
| **Утилиты** | bash 4+, openssl, curl (setup.sh ставит остальное) | то же |

Почему с observability больше: Elasticsearch один просит 1 GB heap (и
ещё столько же off-heap), Grafana/Kibana добавляют ~500 MB суммарно,
плюс место под ingest-путь (filebeat → ES). На 4 GB хосте с включённым
observability получите OOM на любой реальной нагрузке.

### Открыть на firewall хоста **и** в cloud security group

| Порт | Протокол | Назначение |
|---|---|---|
| 80 | TCP | ACME HTTP-01 challenge + редирект на 443 |
| 443 | TCP | HTTPS |
| 4001 | UDP | SFU (видеозвонки) — прямой UDP, через Caddy **не** проксируется |
| 3478 | UDP + TCP | TURN |
| 49152-65535 | UDP | TURN media relays для клиентов за symmetric NAT |

### DNS

Одна A-запись, указывающая на публичный IPv4 хоста. **Должна
резолвиться до запуска `up.sh`** — Caddy на первом старте делает
Let's Encrypt HTTP-01 challenge, и кривой/пустой DNS-ответ упирается в
rate-limits LE на час, после чего сертификат не выпустить.

Проверить: `dig +short ВАШ.ДОМЕН` должен вернуть публичный IP.

---

## Установка

```sh
tar -xzf matehub-box-*.tar.gz
cd matehub-box-*

./setup.sh
# Скрипт спросит:
#   - S3: local (встроенный MinIO) или external (AWS / Yandex / свой bucket)
#   - Observability: yes/no (Grafana + Kibana + Prometheus + Elasticsearch)
#   - DOMAIN, PUBLIC_IP, ACME_EMAIL
# В конце — красная плашка с напоминанием проверить DNS.

./up.sh
# Тянет образы из cr.yandex (по puller.json из архива),
# поднимает стек, ждёт healthcheck'ов (~2 минуты на первом запуске),
# печатает адрес и креды.
```

Открыть `https://<ваш-домен>` в браузере. Первый зарегистрировавшийся
пользователь становится админом хаба — отдельного шага «создать
админа» нет.

---

## Повторный запуск setup или up

Оба скрипта идемпотентны.

* `./setup.sh` после успешного прогона заполняет только **пустые**
  значения в `.env`. Существующие секреты, домен, выбор observability —
  сохраняются. Удобно если решили включить observability позже:
  перезапустить setup, на этот раз ответить "yes".
* `./up.sh` на уже работающем стеке — no-op (compose видит, что
  ничего не изменилось). Запускайте после правок `.env`.

---

## Внешний S3

В диалоге выбрать "external". `setup.sh` оставит S3-поля пустыми —
заполнить вручную в `deploy/.env`:

```
S3_ENDPOINT=https://storage.yandexcloud.net
S3_REGION=ru-central1
S3_PUBLIC_URL=https://storage.yandexcloud.net      # если bucket публичный
S3_ACCESS_KEY_ID=...
S3_SECRET_ACCESS_KEY=...
S3_BUCKET_HUB_ASSETS=matehub-hub-assets
S3_BUCKET_CHAT_MEDIA=matehub-chat-media
```

Если bucket не public-read — поставить `S3_PUBLIC_URL` в CloudFront /
YC CDN endpoint, который его фронтит. Встроенный MinIO в external
режиме не запускается.

---

## Доступ к observability

Когда включён, Grafana и Kibana доступны через Caddy с basic-auth.
Креды печатает `up.sh` и хранит в `deploy/.env`:

* `https://<домен>/grafana/` — дашборды (Prometheus + Elasticsearch
  заведены как datasources). Два слоя авторизации: Caddy basicauth →
  Grafana admin login.
* `https://<домен>/kibana/` — поиск по логам всех matehub-сервисов.
  Filebeat читает `LOG_FORMAT=json` stdout каждого контейнера и шлёт в
  ES; видны индексы по сервису-в-день
  (`matehub-hub-2026.05.10`, `matehub-chat-2026.05.10`, ...).

Чтобы включить in-call отладочную панель видеозвонка на коробке:
открыть в браузере DevTools console и набрать `enableVideoTrace()`.
Страница перезагрузится с панелью диагностики. `disableVideoTrace()`
выключает.

---

## Типичные операции

```sh
# Хвост логов
docker compose -f deploy/docker-compose.yml logs -f hub chat video

# Перезапустить один сервис после правки .env
docker compose -f deploy/docker-compose.yml up -d --force-recreate hub

# Накатить новую версию (после бампа HUB_VERSION и т.д. в .env)
./up.sh

# Остановить, данные сохранить
./down.sh

# Полная зачистка
./down.sh --purge
```

---

## Траблшутинг

**Браузер пишет "your connection is not private"**
Rate-limit Let's Encrypt или DNS не успел прорезолвиться до `up.sh`.
`docker compose logs caddy` скажет что именно. Подождать час,
проверить `dig +short ВАШ.ДОМЕН`, потом
`docker compose restart caddy`.

**Звонки висят на "connecting…"**
Скорее всего TURN. `docker compose logs coturn` плюс проверить, что
firewall открыл `49152-65535/udp`. В debug-панели звонка (см.
`enableVideoTrace()` выше) смотреть `relay` ICE кандидаты — если их
нет, TURN недоступен.

**`hub` отдаёт 500 на login**
`JWT_SECRET` пустой или разный у hub/chat/video. Секрет один на все —
после правки `.env` перезапустить весь стек.

**MinIO загрузка отвечает 200, но файл в чате битый**
Браузер не может достучаться до `http://minio:9000/...`. В этом
релизе пофикшено — backend возвращает `https://<домен>/s3/<bucket>/<key>`,
Caddy проксирует. Если баг ещё виден: убедиться что `S3_PUBLIC_URL` в
`deploy/.env` указывает на публичный путь, и что в Caddy жив маршрут
`/s3/*` (`docker compose logs caddy`).

**OOM на первом старте с observability**
Самый прожорливый — Elasticsearch. Если хост 4 GB — либо отключить
observability (перезапустить setup, ответить "no"), либо нарастить RAM.

---

## Удалить коробку

```sh
./down.sh --purge          # вычистить данные
docker logout cr.yandex    # забыть креды реестра
```

Ключ в `puller.json` имеет только IAM-роль `images.puller` — в худшем
случае при утечке посторонний скачает копию тех же образов, что и у
вас. Прав на push / админство / биллинг нет.
