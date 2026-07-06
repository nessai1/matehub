# RTP-passthrough — HANDOFF (2026-07-01)

Передача незавершённой работы свежей сессии. Всё нужное — здесь.

## Что чиним

SFU видео-сервиса (`services/video`) давал **артефакты видео** (кросс-браузерная
блочная каша при `packetsLost:0`). Диагностировали через ROOM_DEBUG capture
(см. `docs/video/room-debug.md`): str0m в **sample-mode** (`Event::MediaData` +
`Writer::write`) депакетизировал и **заново пакетизировал** кадры на egress,
ломая межкадровые ссылки.

**Решение:** миграция форвардинга на **RTP-passthrough** — `set_rtp_mode(true)`,
`Event::RtpPacket` на входе, `StreamTx::write_rtp` на выходе, релэй payload
издателя 1:1 (никакой пересборки). Это ещё и правильная SFU-архитектура + шаг к
Phase 3 (гибрид) + пререквизит E2EE. Роадмап: `docs/str0m-analysis.md §5`,
`docs/video/currentstate.md` (в шапке есть свежая заметка про этот переход).

## Состояние: незакоммичено, ветка `release/video-stable`, поверх `195ea4f`

Изменены ровно 2 файла (RTP-работа изолирована):
- `services/video/src/sfu/mod.rs` — builder, событийный путь, `forward_rtp`, egress-трейс
- `services/video/src/sfu/session.rs` — `TrackOut.egress_seq_base`, `request_keyframe_throttled`

`cargo build/fmt/clippy -D warnings` — зелёные; 45 lib-тестов проходят.

## Пять слоёв бага, снятых по очереди (каждый — по одному ROOM_DEBUG-звонку)

1. **repacketization → артефакты** — исправлено самим переходом в rtp_mode (байты кадра теперь byte-identical).
2. **PT ingress≠egress → чёрное видео.** Каждый Rtc негоциирует payload type независимо. Фикс: ремап через `target.rtc.codec_config().match_params(source_pp)` → `pp.pt()` в `forward_rtp`. Симптом при баге: `WARN: Media is missing PT (N)`, str0m молча дропает на send.
3. **проброс рваных seq → ложные потери, NACK-шторм** (`packetsLost` растёт лавиной). Фикс: seq-rewrite.
4. **delivery-order renumber ломал out-of-order** — str0m в rtp_mode отдаёт пакеты в порядке ПРИБЫТИЯ (де-RTX'нутые резенды приходят поздно). Счётчик по приходу ломал сборку multi-packet кадров. Фикс: **offset-rewrite** `egress = EGRESS_SEQ_START + (pkt.seq_no − base)`, где `base` = первый seq (`TrackOut::egress_seq_base`). Сохраняет порядок и относительные позиции.
5. **egress-пейсер душил отправку → серый квадрат/фриз.** Egress-трейс показал `n_tx=0` на 95% записей = str0m держал пакеты в leaky-bucket пейсере (без BWE-рейта почти ничего не выпускал). Фикс: `tx.set_unpaced(true)` перед `write_rtp` (SFU релэит уже-отпейсенный поток издателя).
6. **drain-цикл в `forward_rtp` съедал события подписчика** — проверен звонком c01c8f0d: съедание ушло (`video_in == video_out`, PLI доходят). Инлайн-дренаж `target.rtc.poll_output()` после `write_rtp` молча выбрасывал `Output::Event(_)` (`poll_output` — консюмящая очередь, не peek): (а) `Event::RtpPacket` собственного медиа подписчика → ~85% «ingress-потерь»; (б) `Event::KeyframeRequest` (PLI). Фикс: drain-цикл убран; `forward_rtp` кладёт target в `pending_flush`, дренируемый в конце итерации event loop через `flush_pending_writes` → `poll_participant`. `n_tx` из трейса удалён.
7. **сквозной проброс header extensions издателя → Chrome отвязывает SSRC, видео умирает через ~1с** — проверен звонком 7b302c3b: `packetsReceived` растёт (не 1), аудио-дубли исчезли, PLI доходят до издателей (pliCount 8-10, кейфреймы кодируются). `write_rtp(..., pkt.header.ext_vals.clone(), ...)` протаскивал ТРАНСПОРТНЫЕ расширения издателя (sdes:mid «0»/«1», rid, TWCC seq, abs-send-time) в транспорт подписчика. str0m подменяет MID ext только до первого RR по SSRC (`remote_acked_ssrc` в `send.rs`), дальше протухший MID издателя уходит на провод → demuxer Chrome перепривязывает SSRC на transceiver mid «1» подписчика (его СОБСТВЕННЫЙ publish m-line — ответ SFU на join сделал его sendrecv, создав фантомный приёмник) → пакеты не засчитываются: капчур c01c8f0d, подписчик dde6608a: video `packetsReceived:1` при 585 записанных, при этом transport-level `bytesReceived` ~30KB/s (пакеты НА проводе). Фикс: egress `ext_vals` собирается с нуля, копируются только end-to-end значения (audio_level, voice_activity, video_orientation, video_content_type).
8. **батчинг команд затирал str0m `pending_packet` → «зависает, дёргается, зависает»** ← **ПОСЛЕДНИЙ ФИКС, НЕ ПРОВЕРЕН НА ЗВОНКЕ.** В rtp_mode str0m кладёт входящий RTP-пакет в **однослотовый** `pending_packet` (session.rs:490, НЕ очередь) до следующего `poll_output`. `run_blocking` дренировал канал команд пачкой (`try_recv`-цикл) и поллил только после — из N датаграмм одного Rtc в пачке выживала последняя. Seq при этом уже зарегистрирован → **RR рапортует packetsLost:0, «потери» невидимы для NACK/RTX** — SFU честно ретранслировал дыры (~2/3 медиа терялось: капчур 7b302c3b — 1275 событий на span 4026). Диагноз: `remote-inbound-rtp` у клиентов (RR от SFU) = 0 потерь при гигантском packetsLost у получателей → пакеты доходят до str0m и умирают у нас. Фикс: `poll_target` сразу после КАЖДОГО `handle_command`, до следующей команды (контракт str0m: poll до Timeout после каждого handle_input).

## Известные открытые проблемы (не блокеры этого шага)

- **Ответ SFU на join-оффер клиента — `a=sendrecv`** на его publish m-line'ах (mid 0/1) с серверными a=ssrc → фантомные ontrack у клиента (SDK их игнорит: «ignoring ontrack before join completed»). Правильно — SDK должен публиковать через `addTransceiver(track, {direction:'sendonly'})`. После фикса №7 фантомы безвредны, но чистить стоит.
- ~~SFU не шлёт NACK издателям~~ — снято: «ингресс-потери» были артефактом №8 (str0m видел все пакеты, RR=0, NACK'ать было нечего). Реальные потери на LAN ≈ 0. Если на плохой сети NACK-контур всё же не заработает — проверить RTX-маппинг StreamRx (`suppress_nack` в `streams/mod.rs:290`).

## Диагностический приём этого захода

`inbound-rtp.packetsReceived` мёртвый при живом `candidate-pair.bytesReceived` = пакеты доезжают до клиента, но убиваются demux/SRTP-слоем браузера (не считаются в per-SSRC статистике). Смотреть на несоответствие каналов уровня транспорта и уровня SSRC.

## НЕМЕДЛЕННЫЙ следующий шаг

**Проверить фикс №8 живым звонком.** Пользователь запускает
`ROOM_DEBUG=1 cargo run -p matehub-video`, делает звонок 2 участников с камерами,
кидает **call ID**. Проверить в клиентских бандлах: inbound video
`framesDecoded` тикает ~30fps, `packetsLost` ≈ 0 (НЕ сотни), nackCount не
лавинит, картинка субъективно плавная. На сервере: `video rtp in` — in_seq
подряд, без дыр в 3-4.

Если поедет плавно → RTP-passthrough закрыт. Тогда: полный CI-гейт
(`cargo fmt --all -- --check`; `cargo clippy --all-targets --all-features -- -D warnings`;
`cargo test --all -- --test-threads=1` — нужны backing-сервисы `./scripts/dev-up.sh`
+ БД `matehub_test`), обновить `currentstate.md`, предложить коммит (НЕ коммитить без approve).

Если НЕ поедет → egress-трейс (`video rtp out`: `to/in_seq/eg_seq/pt/marker/ts/n_tx`)
+ входной (`video rtp in`: `ssrc/seq/ts/marker/pt`) + клиентские inbound-rtp
покажут причину по фактам.

## Как читать ROOM_DEBUG capture (рабочий процесс отладки)

Пользователь шлёт **call ID** → всё под `room-debug/<call_id>/`:
- Сервер: `grep <call_id> room-debug/server-*.ndjson` (или разбор `jq` по `.message`).
  Полезные сообщения: `video rtp in`, `video rtp out`, `media forwarding stats`
  (поля `video_in/video_out/video_no_pt/video_write_err`), `PLI requested`.
- Клиенты: `room-debug/<call_id>/client-<participant>-<seq>.json` — снимки
  diagnostics (getStats) + лог, раз в ~5с. Ключевые поля inbound-rtp video:
  `packetsReceived, framesReceived, framesDecoded, keyFramesDecoded, packetsLost,
  nackCount, pliCount, freezeCount`.

Диагностический принцип, отработанный в этой сессии: **`packetsLost:0` + артефакты
= SFU отдаёт «чистый по номерам» поток из битых/неполных кадров** (см.
`video_sfu_contiguous_gate` в memory).

## Ключевые факты про str0m 0.7 rtp_mode (проверено по исходникам)

- `Event::MediaAdded` продолжает работать в rtp_mode (треки/mid узнаём как обычно).
- str0m **сам де-RTX'ит**: RTX-пакет переписывается в нормальный (main SSRC/PT,
  восстановленный оригинальный seq) → мой RTX-skip инертен (пакеты всегда main PT).
- Пакеты отдаются в порядке **прибытия** (нет egress-reorder перед выдачей).
- `Event::KeyframeRequest` фаерится в rtp_mode; PLI издателю — через
  `direct_api().stream_rx_by_mid(mid,None).request_keyframe(Pli)` (Writer отключён).
- Egress по умолчанию **пейсится** (leaky bucket) — для SFU-релэя нужен
  `StreamTx::set_unpaced(true)`.
- PT: `Rtc::codec_config()` → `CodecConfig::match_params(PayloadParams)` для ремапа.
- `write_rtp(pt, seq_no, time:u32, wallclock, marker, ext_vals, nackable, payload)`.

## Следующий за этим шаг (НЕ сейчас): simulcast layer-select (step 2)

`selected_rid`/`next_layer`/BWE-логика в `session.rs` уже есть (пока не применяется
в v1). Камера сейчас шлёт один слой (L1T1); screen-share уже 2-слойный. Step 2 —
per-subscriber выбор слоя + RTP munging (seq/pic-id rewrite при переключении на
keyframe). См. обсуждение адаптивного качества в истории.
