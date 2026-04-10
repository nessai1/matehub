# Video Service -- Known Bugs & Required Fixes

## BUG-1: Media forwarding causes growing delay (CRITICAL)

**Симптом:** Со временем задержка между участниками растёт до секунд. Видео/аудио приходит с нарастающим лагом.

**Причина:** str0m требует `poll_output()` после каждого `writer.write()`. Наш код собирает все `Event::MediaData` в вектор, потом пишет их все в цикле forwarding **без** промежуточных `poll_output()`. 

str0m при `write()` ставит пакет в очередь на SRTP-шифрование и отправку. Но пакет не будет отправлен пока не вызван `poll_output()` -- который возвращает `Output::Transmit`. Без poll между write'ами пакеты копятся в очереди, и str0m выдаёт ошибку "Consecutive calls to write() without poll_output() in between".

Мы заменили `disconnect()` на `trace` лог, поэтому соединение не рвётся, но каждый ~20ms приходит новый пакет, а отправляется только один за poll цикл (50ms interval). Результат: очередь растёт, delay увеличивается.

**Как должно работать (по chat.rs example):**

В str0m chat example forwarding происходит **синхронно внутри poll loop**:

```
for client in clients:
    while poll_output(client) != Timeout:
        if MediaData -> propagate to other clients immediately
        if Transmit -> send UDP
```

Каждый participant poll'ится до Timeout, медиа forwarding'ится **inline**, и `poll_output()` для получателя вызывается сразу после `write()`.

**Fix:** Переписать `poll_all_outputs()` чтобы forwarding + poll были interleaved:

```rust
// Для каждого participant:
//   1. poll_output() до Timeout
//   2. Если MediaData -- сразу write() к получателям
//   3. Сразу poll_output() для каждого получателя (чтобы отправить пакет)
//   4. Продолжить poll_output() для текущего participant
```

Это требует переделки двухфазной архитектуры (collect -> forward) в однофазную (poll -> forward -> poll).

**Сложность:** Borrow checker не позволяет одновременно итерировать по `session.participants` (mut) и писать в другого participant'а. Решения:
1. Индексировать по позиции, не по iterator
2. Временно извлекать participant из HashMap через `remove/insert`
3. Использовать `unsafe` split borrow
4. Перенести media data во внешний буфер и обрабатывать в отдельном проходе с poll между write'ами

---

## BUG-2: Second participant doesn't see first participant's video (MEDIUM)

**Симптом:** Второй браузер видит только свою плитку. Не видит камеру первого.

**Причина (frontend):** `pc.ontrack` срабатывает для обоих участников (видно в логах: `ontrack! {kind: 'audio'}, ontrack! {kind: 'video'}`). Но первые два ontrack приходят с `streamCount: 0, streamIds: []` -- это из начального SDP answer (recvonly transceivers). Реальные remote tracks приходят в offer renegotiation с `streamCount: 1`.

Проблема в SDK: `findParticipantByStreamId(streamId)` не находит match потому что `streamId` от str0m (UUID-like строка) не совпадает ни с одним `participantId` в `this.participants` Map.

В str0m chat example `stream_id` устанавливается как `origin.to_string()` (client ID). В нашем коде мы передаём `track.origin.to_string()` -- это `ParticipantId` (UUID). Но фронт ищет match по `participantId` из `participant_joined` event -- который приходит с другим UUID.

**Fix (backend):** При `add_media()` в renegotiation, `stream_id` должен быть стабильным идентификатором origin participant'а, и фронт должен использовать этот же ID.

**Fix (frontend):** `ontrack` handler должен маппить stream по `stream.id` к participant. Нужно передавать mapping `stream_id -> participant_id` через signaling, или использовать `participant_id` как `stream_id` при `add_media()`.

---

## BUG-3: ICE Disconnected after Completed (LOW)

**Симптом:** ICE переходит Connected -> Completed -> Disconnected. Медиа перестаёт flowing.

**Причина:** str0m продолжает пробовать alternative candidate pairs после nomination. Когда все non-nominated pairs fail'ятся, str0m переходит в Disconnected -- даже если nominated pair работает.

**Fix:** Не реагировать на ICE Disconnected (уже сделано в текущем коде). В будущем -- настроить str0m ICE agent чтобы не пробовать лишние pairs после Completed.

---

## Architecture Issues Found in Code Review

### ISSUE-1: Single-pass forwarding with deferred poll (causes BUG-1)

`poll_all_outputs()` в `sfu/mod.rs` имеет двухфазную архитектуру:
1. Phase 1: Poll все Rtc, собрать MediaData/tracks_opened в Vec
2. Phase 2: Forward MediaData к получателям (write)
3. Phase 3: Negotiate new tracks

Проблема: Phase 2 вызывает `write()` N раз без `poll_output()` для получателей. Нужно либо interleave, либо после каждого write делать mini-poll для получателя.

### ISSUE-2: `media_to_forward` holds borrowed data across mutable iteration

```rust
let mut media_to_forward: Vec<(SessionId, ParticipantId, MediaData)> = Vec::new();
// ... collect from all participants ...
// ... then iterate sessions mutably to write ...
```

`MediaData` содержит `data: Vec<u8>` -- owned, не borrowed. Но итерация по `self.sessions` для записи требует `&mut self`, а `media_to_forward` уже содержит `SessionId` ключи. Это работает, но неэффективно -- каждый MediaData клонируется (`data.data.clone()`).

### ISSUE-3: No poll_output for receiving participant after write

Критическая проблема: после `writer.write(pt, ...)` нужно вызвать `rtc.poll_output()` для получателя чтобы str0m подготовил пакет к отправке. Без этого пакет остаётся в очереди.

**Recommended fix pattern (from str0m chat example):**

```rust
// Process one participant at a time
for i in 0..participant_count {
    let participant = &mut participants[i];
    
    loop {
        match participant.rtc.poll_output()? {
            Output::Transmit(t) => { socket.send_to(...); }
            Output::Timeout(t) => { break; }
            Output::Event(Event::MediaData(data)) => {
                // Forward to other participants IMMEDIATELY
                for j in 0..participant_count {
                    if i == j { continue; }
                    let other = &mut participants[j];
                    // find matching track_out, write
                    if let Some(writer) = other.rtc.writer(mid) {
                        writer.write(pt, ...)?;
                        // IMMEDIATELY poll the other participant
                        // to flush the written packet
                        drain_transmits(&mut other.rtc, &socket);
                    }
                }
            }
            _ => {}
        }
    }
}

fn drain_transmits(rtc: &mut Rtc, socket: &UdpSocket) {
    loop {
        match rtc.poll_output() {
            Ok(Output::Transmit(t)) => { socket.send_to(...); }
            Ok(Output::Timeout(_)) => break,
            Ok(Output::Event(_)) => {} // ignore events during drain
            Err(_) => break,
        }
    }
}
```

Проблема: Rust borrow checker не позволяет `&mut participants[i]` и `&mut participants[j]` одновременно. Решения:

1. **split_at_mut:** `let (left, right) = participants.split_at_mut(j);`
2. **IndexMap + index-based access:** Вместо HashMap использовать Vec или IndexMap
3. **Cell/RefCell:** Wrap Rtc в RefCell (не идеально для performance)
4. **Two-pass с drain:** Phase 1 poll + collect, Phase 2 write + drain per recipient

Рекомендация: **вариант 4** -- после каждого `write()` вызывать `drain_transmits()` для получателя. Это добавляет вложенный цикл, но решает проблему delay.
