# ROOM_DEBUG — per-call capture

A debugging aid for the video service: when enabled, the service persists a
full per-call debug bundle — **server-side tracing** for the run plus **each
client's debug-panel snapshot** (diagnostics + log ring buffer) — keyed by the
**call id** (the session UUID shown in the client debug panel).

The point: to investigate a bad call you only need to share the **call id**.
Everything needed to reconstruct it (both sides) is on disk under that id.

## Enabling

Start the video service with:

| Env | Default | Meaning |
|-----|---------|---------|
| `ROOM_DEBUG=1` | _(off)_ | Turn capture on. |
| `ROOM_DEBUG_DIR` | `room-debug` | Capture root directory. |
| `ROOM_DEBUG_FILTER` | `matehub_video=debug,str0m=warn` | `EnvFilter` for the **capture file only**. stdout keeps its usual `info` verbosity. |

```bash
ROOM_DEBUG=1 cargo run -p matehub-video
```

Off by default → zero overhead and the tracing setup is byte-for-byte
unchanged (`init_tracing` vs `init_tracing_with_capture(None)`).

## What gets written

```
room-debug/
  server-<run>.ndjson              # every server tracing event (debug level),
                                   #   one JSON object per line, span fields
                                   #   (call_id, session_id, source_pid, …)
                                   #   hoisted to top level
  <call_id>/
    client-<participant>-1.json    # client A's bundle, upload #1
    client-<participant>-2.json    # client A's bundle, upload #2 (later)
    client-<participant>-1.json    # client B …
```

- **Server log** — a debug-level JSON tee of the service's own tracing,
  written via a dedicated writer thread (drop-on-full, never blocks the tokio
  runtime or the media shard threads). One file per process run.
- **Client bundles** — each client uploads its debug snapshot (the same JSON
  the debug panel's "Copy" button produces) to
  `POST /v1/sessions/{session_id}/debug` every ~5s while in the call, plus a
  final flush on leave / tab close. The server writes them under the call dir,
  numbered per participant so the timeline is reconstructable.

The client only uploads when the server advertised capture: the
`POST /v1/sessions` response carries `debug_capture: true` under `ROOM_DEBUG`,
and the SDK uploads only then. A normal deployment sees no extra traffic and
the endpoint returns `404`.

## Investigating a call

Given a call id `3155b42b-…`:

```bash
# Server side — every event tagged with that call:
grep 3155b42b room-debug/server-*.ndjson

# Client side — both participants' diagnostics + logs over time:
ls    room-debug/3155b42b-5cd5-4145-9746-451e04c0d623/
cat   room-debug/3155b42b-.../client-*.json | jq .
```

The client bundles list the participant UUIDs; use them to also grep the
server ndjson for SFU/media-thread events (which are keyed by `source_pid`
rather than `call_id`).

## Endpoint

`POST /v1/sessions/{session_id}/debug`

- Body: the client debug bundle JSON (`{ userId, sessionId, participantId,
  timestamp, userAgent, diagnostics, logs }`). Body-limited to 4 MiB.
- `204 No Content` on success (fire-and-forget; the writer thread does I/O).
- `404 Not Found` when `ROOM_DEBUG` is off.

## Notes / limits

- Debug-only. Not meant for production capture — files are unbounded on disk
  (one ndjson per run + a dir per call); clean up `room-debug/` periodically.
  It is gitignored.
- The participant token in client bundle filenames is sanitized
  (`[A-Za-z0-9-]`, length-bounded) before touching the filesystem.
- Capture overflow (writer can't keep up) is counted in the
  `matehub_video_room_debug_drops_total` metric rather than back-pressuring
  the call.
