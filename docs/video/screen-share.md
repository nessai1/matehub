# Screen Share — Technical Design & Plan

> **Status:** Phase 1 (MVP video + opportunistic audio) shipped and verified
> on two browsers. Phases 2–5 open.
>
> Revision log:
> - **2026-04-18**: Phase 1 landed. Decision #3 reversed (self-preview is
>   now local-only, not SFU round-trip) — rationale in §12.

---

## 1. Use case & implications

**Who**: boosters watching gameplay demos together and discussing them on-call.

**What they share**: recorded or live game footage, often a native app (game
client) or a browser tab (YouTube/Twitch). Occasionally a document or editor.

**Why this matters for the design**:

| Property | Value | Design implication |
|---|---|---|
| Content type | **motion-heavy** (30–60fps gameplay) | High bitrate budget (4–8 Mbps), `contentHint: "motion"`, NOT the "screen = low-fps docs" default |
| Viewer expectation | **low latency** (<300 ms glass-to-glass) so verbal commentary stays in sync | Tight jitter buffer, `playoutDelayHint`, no sub-second CDN detour |
| Audio | **system audio** (game sounds), not mic-only | `getDisplayMedia({audio:true})`, platform caveats (see §4) |
| Concurrency | camera + screen at the same time (face-cam while sharing) | Two video tracks from one publisher — not a switch |
| Session length | 20–60 min typical | Memory/leak budget matters; RTX cache tuning |
| Failure modes | user hits "Stop sharing" in browser UI, monitor unplugs, permission revoked | `track.onended` → clean unpublish path, renegotiation |

The boosters story is specifically *not* "office screen share of documents".
It's closer to game streaming to an audience of 1–5. The defaults industry
tooling picks (VP8 @ 500 kbps, 5 fps, `contentHint: "detail"`) will look awful
for this audience.

---

## 2. Platform primitives

### 2.1 `getDisplayMedia`

```ts
const stream = await navigator.mediaDevices.getDisplayMedia({
  video: {
    frameRate: { ideal: 30, max: 60 },
    width: { ideal: 1920, max: 2560 },
    height: { ideal: 1080, max: 1440 },
  },
  audio: true,                  // system/tab audio — see §4
  // Chrome-only options:
  selfBrowserSurface: "exclude", // don't show "this tab" in picker
  surfaceSwitching: "include",   // let user switch source mid-share
  systemAudio: "include",        // Windows: include desktop audio
});
```

Returns `MediaStream` with 1 video track and 0–1 audio tracks. The audio
track is `null` on macOS unless the user picked a browser tab as the source.

### 2.2 `contentHint`

```ts
videoTrack.contentHint = "motion"; // or "detail" / "text"
```

- `"motion"` — gaming, video playback. Encoder prioritises smooth motion over
  per-frame sharpness. Tuned for 30/60fps.
- `"detail"` — documents, IDEs. Encoder prioritises sharpness of static
  regions (text readable), accepts low framerate.
- `"text"` — heavy static, text-only. Even more aggressive static
  optimisation.

**We default to `"motion"`** for boosters. Expose a UI toggle for the rare
"sharing a spreadsheet" case later.

### 2.3 Codec selection

str0m negotiates VP8/VP9/H.264/AV1 from the client offer. For screen:

| Codec | Quality @ gaming | HW encode | HW decode | Verdict |
|---|---|---|---|---|
| **VP9** | Best (alt-ref frames, dynamic res) | Chrome/Edge on modern CPUs | Universal | **Default** |
| VP8 | Acceptable | Universal | Universal | Fallback |
| H.264 | Decent | macOS/iOS HW | Universal | iOS Safari fallback |
| AV1 | Best bitrate efficiency | Limited (Intel 11th+, ARM v9) | Chrome 100+ | Future, not MVP |

str0m picks by SDP priority. We don't intervene.

### 2.4 Encoder bitrate knobs

Chrome's default for screen is conservative. Override via RTCRtpSender:

```ts
const params = videoSender.getParameters();
params.encodings[0].maxBitrate = 6_000_000;   // 6 Mbps for gaming
params.encodings[0].maxFramerate = 60;
params.encodings[0].scaleResolutionDownBy = 1; // native res
await videoSender.setParameters(params);
```

Applied after the transceiver is created, before renegotiation completes.

### 2.5 Latency tuning on receiver

```ts
receiver.playoutDelayHint = 0.05; // 50 ms target buffer
// (experimental API, Chrome only, but no-op silently elsewhere)
```

Trades dropout tolerance for latency. Worth it for interactive review.

---

## 3. Why a separate track, not a separate PeerConnection

Two options:

**(A) Separate PC for screen** — each participant has 2 PCs: one for camera/mic,
one for screen.
- Pros: isolation (screen fail ≠ camera fail), clean bandwidth accounting.
- Cons: 2× ICE/DTLS handshake, 2× SRTP contexts, 2× signaling, 2× memory per
  participant on SFU. Doubles our per-client overhead.

**(B) Extra `m=` section in the same PC** — one sendrecv PC, but the publisher
adds a third+fourth transceiver (screen video, optionally screen audio).
- Pros: single ICE/DTLS path, single TWCC congestion control, half the state.
- Cons: TWCC now juggles camera+screen over one transport — a spike on screen
  can throttle camera. Mitigated by simulcast layer selection per track.

**Decision: (B)**. str0m handles multiple `m=` per Rtc cleanly; our existing
pre-computed `forwarding_map` keys on `(publisher_pid, source_mid)` — adding
a second video source is a zero-line structural change to the map.

---

## 4. Platform limitations for system audio

This is the nastiest part of the feature. There is no cross-platform way to
capture arbitrary OS audio from a browser.

| OS + Browser | What `getDisplayMedia({audio:true})` captures |
|---|---|
| **Windows + Chrome** | Entire desktop or any app window — works |
| **Windows + Firefox** | Same, works |
| **Windows + Edge** | Same, works |
| **macOS + Chrome** | **Only browser tab audio**, not system/app audio |
| **macOS + Safari** | No audio via getDisplayMedia at all |
| **Linux + Chrome** | Depends on PipeWire/PulseAudio routing; usually tab-only |
| **Mobile browsers** | No system audio capture at all |

**For native-game boosters on macOS** the out-of-the-box answer is: no audio.
Workarounds (listed, not recommended):
1. Install a virtual audio device (BlackHole / Loopback) that routes system
   output to an input device, then `getUserMedia` picks that up as "mic".
2. Use the native Mac app version of the platform (future).

**MVP response**: on macOS show an inline warning "Game audio capture not
supported — use the Windows client or a virtual audio device" next to the
Start Sharing button if we detect `navigator.userAgent.includes("Mac")`. Don't
block the feature — video-only screen share is still valuable.

---

## 5. End-to-end architecture

```
┌─────────────────────────────────────────────────────────────────┐
│ Publisher browser                                               │
│                                                                 │
│   <button Start sharing> ───► getDisplayMedia({video, audio})   │
│                                        │                        │
│                                        ▼                        │
│   MediaStreamTrack video  ──► pc.addTrack(video, …)             │
│   MediaStreamTrack audio? ──► pc.addTrack(audio, …)             │
│                                        │                        │
│                              (re-negotiation)                   │
│                                        ▼                        │
│          WS signal {type: "publish_track", source: "screen",    │
│                     has_audio: true/false}                      │
└──────────────────────────┬──────────────────────────────────────┘
                           │
                           ▼
┌─────────────────────────────────────────────────────────────────┐
│ SFU (services/video)                                            │
│                                                                 │
│   handle_answer: track transitions to Open(mid)                 │
│        ↓                                                        │
│   SfuParticipant.tracks_in.push(TrackIn {                       │
│       mid, kind: Video, source: Screen                          │
│   })                                                            │
│        ↓                                                        │
│   Queue TrackOut { source: Screen } for every OTHER participant │
│   has_pending_negotiation = true                                │
│        ↓                                                        │
│   negotiate_pending_tracks(): offer with msid =                 │
│        "<publisher-uuid>-screen-video" / "-screen-audio"        │
│        (separate msid so Chrome doesn't A/V-sync screen with    │
│         an absent camera track)                                 │
│        ↓                                                        │
│   forwarding_map[(publisher, screen_mid)] = [                   │
│       (sub_A, sub_A_screen_mid),                                │
│       (sub_B, sub_B_screen_mid), …                              │
│   ]                                                             │
└──────────────────────────┬──────────────────────────────────────┘
                           │ RTP forwarding (existing hot path)
                           ▼
┌─────────────────────────────────────────────────────────────────┐
│ Subscriber browsers                                             │
│                                                                 │
│   ontrack with streamId = "<publisher>-screen-video"            │
│        ↓                                                        │
│   SDK emits TrackAdded { source: "screen", kind: "video" }      │
│        ↓                                                        │
│   CallGrid layout:                                              │
│       ┌───────────────────────────────────┐                     │
│       │                                   │                     │
│       │       SCREEN (full width)         │                     │
│       │                                   │                     │
│       ├─────┬─────┬─────┬─────────────────┤                     │
│       │ cam │ cam │ cam │ …               │                     │
│       └─────┴─────┴─────┴─────────────────┘                     │
└─────────────────────────────────────────────────────────────────┘
```

### 5.1 Why new `-screen-` msid suffix

The existing `stream_id_for(origin, kind)` returns `<uuid>-audio` / `<uuid>-video`.
If a publisher sends both camera *and* screen, both show up as `-video` and
Chrome lumps them into one MediaStream — same A/V-sync bug we fixed for
mic+camera. We need a third dimension: source.

New signature:

```rust
fn stream_id_for(origin: ParticipantId, source: Source, kind: MediaKind) -> String {
    let source = match source { Source::Camera => "cam", Source::Screen => "screen" };
    let kind   = match kind   { MediaKind::Audio => "audio", MediaKind::Video => "video" };
    format!("{origin}-{source}-{kind}")
}
```

Result: `a32d…-cam-video`, `a32d…-screen-video`, `a32d…-screen-audio`. All
RFC-7941-safe (dashes only).

### 5.2 Signaling protocol changes

Client → Server, new message:

```jsonc
{
  "type": "publish_track",
  "source": "screen",          // "camera" | "screen"
  "kind": "video",             // "audio" | "video"
  "track_id": "<mst.id>",      // client-side track id for correlation
  "content_hint": "motion"     // optional
}
```

And:

```jsonc
{ "type": "unpublish_track", "track_id": "<mst.id>" }
```

Server → Client offer adds `source` to TrackMapping:

```jsonc
{
  "type": "offer",
  "sdp_offer": "…",
  "tracks": [
    {
      "stream_id":     "a32d…-cam-video",
      "participant_id":"a32d…",
      "user_id":       "alice",
      "source":        "camera",
      "kind":          "video"
    },
    {
      "stream_id":     "a32d…-screen-video",
      "participant_id":"a32d…",
      "user_id":       "alice",
      "source":        "screen",
      "kind":          "video"
    }
  ]
}
```

This makes the SDK's `TrackAdded` event carry source info, so the UI can
decide layout without heuristics.

### 5.3 Data-model changes (backend)

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source { Camera, Screen }

pub struct TrackIn {
    pub mid: Mid,
    pub kind: MediaKind,
    pub source: Source,   // NEW
}

pub struct TrackOut {
    pub origin: ParticipantId,
    pub origin_mid: Mid,
    pub kind: MediaKind,
    pub source: Source,   // NEW — passed through from TrackIn
    pub state: TrackOutState,
}
```

`forwarding_map` key stays `(ParticipantId, Mid)` — the (source, kind) pair
is derivable from the TrackIn entry if needed. No structural change there.

---

## 6. Quality profiles & simulcast layers

Every screen share ships as **two simulcast layers** — a full-quality `high`
and a 720p `low` fallback. The profile picked by the publisher controls the
`high` layer's encoding envelope; `low` is fixed across profiles.

### 6.1 `high` layer (profile-dependent)

Presets applied via `setParameters` on the sender after `addTrack`. User-
selectable in UI each time (non-sticky, see §12 decisions). Default: Gaming.

| Profile | Resolution | Framerate | maxBitrate | contentHint |
|---|---|---|---|---|
| **Gaming** (default) | 1080p (ideal), 1440p cap | 30 ideal, 60 max | 6 Mbps | motion |
| **Standard** | 720p | 24 | 2 Mbps | motion |
| **Detail** (docs) | Native (up to 1440p) | 5 | 1 Mbps | detail |

### 6.2 `low` layer (fixed)

Always published alongside `high`:

| Field | Value |
|---|---|
| Resolution | 720p (or `scaleResolutionDownBy: 2` from source if source is <720p) |
| Framerate | 15 fps |
| maxBitrate | 400 kbps |
| contentHint | same as `high` |

Sized so a viewer on a constrained uplink (mobile LTE, packet loss) still
gets readable motion at ~400 kbps. The `high` → `low` ratio is ~15× which
matches what Meet publishes for screen.

### 6.3 SDP `addTransceiver` shape

```ts
const transceiver = pc.addTransceiver(videoTrack, {
  direction: "sendonly",
  sendEncodings: [
    { rid: "h", maxBitrate: 6_000_000, maxFramerate: 60 },
    { rid: "l", maxBitrate: 400_000, maxFramerate: 15, scaleResolutionDownBy: 2 },
  ],
});
```

Chrome encodes both layers in parallel. Publisher CPU cost for 1080p60
gaming with simulcast is roughly 1.25–1.4× the single-layer cost (the low
layer encodes from the downscaled source, cheap).

### 6.4 SFU layer selection

str0m exposes incoming RTP per `rid`. In `forward_media_now`, for each
subscriber we decide which layer to forward:

```
target_layer(subscriber) =
    if subscriber.bandwidth_estimate < 1 Mbps       → "l"
    else if subscriber.bandwidth_estimate < 3 Mbps  → "l"  // conservative
    else                                            → "h"
```

Implementation implication: `forwarding_map` value gains a per-target
current-layer field, updated from TWCC estimates. Layer switch triggers a
PLI request on the new layer so the subscriber gets an IDR. Hysteresis (wait
N seconds before switching back up) to avoid flapping during borderline
bandwidth.

**MVP can ship with static layer = `high`** and no adaptation logic; that's
feature-equivalent to single-layer. The adaptation pass lands in Phase 5
and is where the simulcast investment actually pays off.

---

## 7. Fault tolerance

### 7.1 "Stop sharing" (browser UI) → `track.onended`

```ts
track.onended = () => void this.unpublishScreenShare();
```

`unpublishScreenShare()`:
1. `pc.removeTrack(videoSender)` + audio if present
2. `track.stop()` to release OS resources
3. `send({type: "unpublish_track", track_id})`
4. Trigger renegotiation (client-initiated since we're removing)
5. Clear UI preview

SFU on `unpublish_track`:
1. Find `TrackIn` by publisher + mid, mark as pending-remove
2. `set_direction(Inactive)` on every subscriber's TrackOut with this origin
3. Renegotiate → subscribers see the transceiver go inactive, tile flips back
   to grid layout

### 7.2 Monitor unplugged / source gone

Same path — Chrome fires `onended` automatically when the capture source
disappears. No extra handling needed.

### 7.3 Permission revoked mid-call

macOS Screen Recording permission can be revoked from System Settings. Chrome
fires `onended` with no special error. Same cleanup path.

### 7.4 Reconnect / SFU restart

The key invariant: `MediaStreamTrack` instances are client-memory objects,
not network-bound. A transient ICE disconnect or a full SFU-side reboot does
**not** kill the track. That lets us silently resume, within limits.

Three cases:

| What happened | Track status | Action |
|---|---|---|
| Transient ICE blip (route flap, brief loss) | Live — PC recovered on its own | Nothing. Already handled by not treating `connectionState === "disconnected"` as fatal (#13). |
| SFU reboot or client-side rejoin, tab intact | `track.readyState === "live"` | **Auto-resume**: `pc.addTrack(existingTrack, …)` on the new PC, re-send `publish_track`, renegotiate. No user prompt. |
| Tab reload, browser close, explicit `track.stop()`, or "Stop sharing" in Chrome UI | `track.readyState === "ended"` | Can't reacquire silently — `getDisplayMedia` requires a user gesture. Show a non-modal toast "Screen share stopped on disconnect — [Resume]" and let the user click. |

Pseudocode in the SDK reconnect path:

```ts
async onReconnect() {
  // … re-establish WS, recreate PeerConnection, re-add local mic/camera …

  if (this.screenVideoTrack?.readyState === "live") {
    this.pc.addTrack(this.screenVideoTrack, this.screenStream);
    if (this.screenAudioTrack?.readyState === "live") {
      this.pc.addTrack(this.screenAudioTrack, this.screenStream);
    }
    this.send({ type: "publish_track", source: "screen", kind: "video", … });
    // SFU answers, forwarding resumes. No user interaction.
  } else if (this.screenVideoTrack) {
    // Track died during the disconnect window. Ask the user to reauthorise.
    this.emit({
      type: "screen_share_interrupted",
      reason: "track_ended_during_reconnect",
    });
    this.screenVideoTrack = null;
  }
  // else: user wasn't sharing, nothing to resume.
}
```

In practice this means: **network hiccup or SFU restart → demo continues
transparently; only an explicit browser-level stop requires a manual resume**.

### 7.5 Keyframe request after subscriber join

Already handled: `handle_answer` issues a PLI when a TrackOut transitions
to Open. Same code path works for screen without modification.

### 7.6 Screen track bitrate spikes crowd out camera

Shared TWCC means a 6 Mbps screen spike can starve a 1 Mbps camera. Two
mitigations:
1. Per-track `maxBitrate` via `setParameters` — a hard ceiling so screen
   can't consume everything.
2. (Phase 5) Simulcast: SFU downgrades the screen layer for constrained
   subscribers without touching the publisher's encoding.

---

## 8. UI/UX

### 8.1 Publisher side

- **Start Sharing** button in CallControls (already exists, currently wired to
  the stub). On click:
  1. Pre-flight: show modal "Gaming / Standard / Detail" profile picker with
     a "don't ask again" checkbox.
  2. Call `getDisplayMedia`. Browser shows its native picker.
  3. On success: replace the camera tile with a small "you are sharing"
     preview; screen tile takes the main stage for self too (same as what
     subscribers see, so publisher sees what they're showing).
  4. Change button label to **Stop Sharing** + red.
- **Stop Sharing** → `unpublishScreenShare()`.
- macOS warning banner below the button if audio capture will be limited.

### 8.2 Subscriber side

- Layout switch: when any participant has `source: "screen"` active, the
  grid becomes:
  ```
  +------------------------+
  |                        |
  |   SCREEN (primary)     |
  |                        |
  +----+----+----+---------+
  | A  | B  | C  |  D      |   ← camera tiles, reduced size
  +----+----+----+---------+
  ```
- Name badge on the screen tile: "Alice's screen" with a small monitor icon.
- Audio from the screen routes to the existing per-participant `<audio>` sink
  (dedicated element already added in Phase-A fix), indistinguishable from
  mic audio output-wise.

### 8.3 Only one screen at a time?

**Not enforced** at MVP. Multiple screen shares produce multiple primary
tiles. UI stacks them vertically. Edge case, low priority. If it becomes a
problem we can add an SFU-side lock later.

---

## 9. Phased implementation plan

### Phase 1: MVP screen share — video + opportunistic audio ✅ DONE (2026-04-18)

- [x] Backend: `enum Source { Camera, Screen }` + `Source::as_msid_tag()`;
      added to `TrackIn`, `TrackOut`, and `TrackMapping` (serialized as
      `"camera"` / `"screen"` + `"audio"` / `"video"` over WS).
- [x] Backend: `stream_id_for(origin, source, kind)` — three-component
      `<uuid>-<cam|screen>-<audio|video>` format, RFC 7941 msid-safe.
- [x] Backend: `ClientMessage::PublishTrack { source, kind, track_id }` +
      `ClientMessage::Offer { sdp_offer }` (client-initiated renegotiation
      path — previously all offers were server-initiated).
      `SfuCommand::PublishTrack` + `SfuCommand::ClientOffer` with
      `handle_publish_track` / `handle_client_offer` in the engine.
- [x] Backend: `SfuParticipant.pending_source_hints:
      HashMap<MediaKind, VecDeque<Source>>`. `publish_track` signaling
      pushes onto the queue; `MediaAdded` pops FIFO by kind and defaults to
      `Camera` on miss (preserves the existing Join flow).
- [x] Backend: simulcast `rid` wiring. `forward_media_now` parses
      `data.rid` (`Rid` derefs to `str`), forwards only the `"h"` layer;
      `"l"` packets drop silently. Subscriber-side adaptation is Phase 5
      territory; for now high quality lands on everyone.
- [x] SDK: `PROFILE_CONFIG` presets (Gaming / Standard / Detail) with
      `width`, `height`, `fps`, `maxBitrate`, `contentHint`.
- [x] SDK: `publishScreen(profile)` — `getDisplayMedia` with profile-
      driven constraints, `contentHint`, `addTransceiver` with two
      `sendEncodings` (h 6 Mbps / l 400 kbps + `scaleResolutionDownBy`),
      separate audio sender when the OS allowed capture, two
      `publish_track` hints, client-initiated Offer.
- [x] SDK: `unpublishScreen()` — `removeTrack` on both senders, `stop()`,
      renegotiate. `track.onended` on the video track wires to
      `unpublishScreen` so "Stop sharing" in Chrome's bar cleans up.
- [x] SDK: `Participant` shape gains `screenVideoTrack` + `screenAudioTrack`.
      New events: `track_added { source, kind }`, `track_removed`,
      `screen_share_started`, `screen_share_stopped`. `streamMeta` map
      (`stream_id → {source, kind}`) populated from `offer.tracks[]` so
      `ontrack` routes to the correct participant slot.
- [x] SDK: `VideoClient.getScreenVideoTrack()` / `getScreenAudioTrack()`
      for local self-preview (added after we discovered the SFU doesn't
      loop the publisher's stream back — see §12 decision #3).
- [x] Frontend: `useVideoClient` exposes `publishScreen`,
      `unpublishScreen`, `isScreenSharing`, `localScreenVideoTrack`.
- [x] Frontend: `ScreenShareProfileDialog` — profile picker modal, macOS
      banner about limited system-audio capture.
- [x] Frontend: `CallGrid` switches to pinned layout the moment any
      participant (local or remote) has a `screenVideoTrack`. Screen tile
      uses `object-contain` + hidden `<audio>` sink for system audio.
      Camera tiles collapse to a bottom strip with `max-h-32`.
- [x] Frontend: Self-preview tile is rendered first from the local
      `MediaStreamTrack` (audio track intentionally null to avoid echo);
      remote screens follow from `participants[].screenVideoTrack`.
- [x] Frontend: `CallControls` — `Monitor` / `MonitorOff` icon toggle
      with red tint while sharing, opens the dialog on start.
- [x] Unit tests: `stream_id_for` 4-combo matrix (source × kind),
      msid-safe charset, deterministic, unique per publisher. `TrackOut`
      carries `source` through the constructor.
- [x] All pre-existing tests still green (12 passed, 0 regressions).

### Phase 2: System audio

- [ ] SDK: `getDisplayMedia({audio:true})`; if returned, `addTrack` as a
      second sender, `publish_track` with `kind: "audio", source: "screen"`.
- [ ] SDK: UA detection banner for macOS warning.
- [ ] Backend: already works — audio track is just another TrackIn with
      source=Screen.
- [ ] Frontend: audio level indicator on the screen tile (reuse existing
      per-participant analyser, keyed by track).

### Phase 3: Quality tuning

- [ ] `setParameters({maxBitrate, maxFramerate, scaleResolutionDownBy})`
      per profile.
- [ ] `contentHint` per profile.
- [ ] `playoutDelayHint = 0.05` on screen receivers.
- [ ] Debug panel: add screen-track row (bitrate, fps, resolution).

### Phase 4: Fault tolerance

- [ ] `track.onended` → `unpublishScreen()`.
- [ ] Renegotiation flow on client-initiated removeTrack (currently server
      initiates all renegotiations; client-initiated needs a new offer path
      in SDK + handle_offer_from_client in SFU).
- [ ] Toast on ICE failure with "Rejoin" CTA.

### Phase 5: Simulcast layer adaptation

The two-layer simulcast config lands together with Phase 1 (it's a field in
`addTransceiver`). This phase is **just** the SFU-side selection logic — the
per-subscriber bandwidth tracking and the PLI/keyframe flow on layer switch.

- [ ] Parse incoming `rid` in `Event::MediaData` (str0m exposes it).
- [ ] `forwarding_map` value extended with `current_layer: Rid` per target.
- [ ] TWCC bandwidth estimate per-subscriber (we already receive TWCC
      feedback; wire it to a per-participant estimator).
- [ ] Layer selection policy: <1 Mbps → `l`, ≥3 Mbps → `h`, hysteresis on
      switch-up (2s cooldown).
- [ ] On layer switch: PLI for the new layer's origin, set
      `current_layer` after the first keyframe arrives.
- [ ] Debug panel: add per-target `current_layer` + estimate.

### Phase 6 (stretch): Hardware encoder hints

- [ ] VP9 hardware acceleration flags in the offer (Chrome-specific).
- [ ] AV1 opt-in for low-bandwidth, high-efficiency paths.

---

## 10. Testing strategy

### 10.1 Unit tests

- `stream_id_for` — 4-case matrix.
- `TrackIn`/`TrackOut` serialization round-trip including source.
- `forwarding_map` update/drop symmetric for screen tracks.
- Signaling message parsing: `publish_track` with / without `source`.

### 10.2 Integration (manual for MVP)

Test matrix (minimal):

| OS | Browser | Sharing | Expected |
|---|---|---|---|
| macOS | Chrome | Full screen, video only | Works, no audio warning dismissed |
| macOS | Chrome | Browser tab (YouTube) + audio | Works, audio too |
| Windows | Chrome | Full screen + desktop audio | Works including audio |
| Windows | Firefox | Full screen | Video works, verify audio limits |
| Linux | Chrome | Full screen | Video works |

### 10.3 Latency

Use the debug panel — measure `currentRoundTripTime` stays <10ms LAN while
sharing. Visually: alt-tab on the publisher, measure mirror lag on receiver.
Target: <300 ms perceived. Benchmark with and without `playoutDelayHint`.

### 10.4 Endurance

10-minute continuous gaming screen share, two receivers. Watch for memory
growth (RTX buffer, forwarding_map churn), packet loss, frame drops.

---

## 11. Known limitations / deferred

- **No cursor-in-stream highlighting** — Chrome includes cursor by default,
  not configurable via Web API without the old deprecated options.
- **No "share a region"** — getDisplayMedia is full-surface only.
- **Mobile publishers won't work** — mobile browsers don't expose
  getDisplayMedia. Acceptable for boosters (desktop audience).
- **macOS system audio** — see §4, out of scope for MVP.
- **Automatic resume after tab reload / explicit stop** — `getDisplayMedia`
  is gated on a user gesture, so a dead track can only be re-acquired by a
  click. Transparent auto-resume works for the common case (network blip,
  SFU restart) as long as the tab is intact — see §7.4.
- **More than one concurrent sharer** — allowed but UI is not optimized.

---

## 12. Decisions (previously open questions)

Recorded here so future contributors see why each knob sits where it does.

1. **Profile picker every Start Share, not sticky.** Boosters switch
   contexts (native game vs. browser tab vs. IDE code review) often enough
   that a persistent "same as last time" would bite. Cost: one extra click
   per share session, acceptable.
2. **No auto-mute of the mic when screen-audio is active.** The two senders
   are independent; the publisher keeps both on and talks over the game
   audio (the primary boosting workflow). Echo via speakers-into-mic is a
   headphones-or-not problem on the publisher's side, not something we
   mitigate server-side.
3. **Publisher sees their own screen as a local-only preview** (reversed
   from the initial "via SFU round-trip" plan — see revision note below).
   The SFU doesn't loop a publisher's stream back to themselves, so the
   self-tile reads the `MediaStreamTrack` returned by `getDisplayMedia`
   directly. Upside: no extra forwarding load, no duplicate decode, and
   we avoid the trap of the publisher seeing their own capture with
   simulcast-`low` quality. Downside: the preview is pre-encoder — you
   don't see encoder artifacts or frame drops the way viewers do. For
   booster-demo QA that's acceptable; if it becomes a pain point a
   stats-panel check is cheaper than changing architecture.
4. **Simulcast: 2 layers** — `high` (profile-driven, up to 6 Mbps, 60fps)
   + `low` (720p, 400 kbps, 15 fps). Three layers tripled encoder load on
   60fps gaming content and only helped a narrow "middle viewer" band that
   we don't expect to populate in the booster audience. Two layers cover
   "on a good desktop" vs. "on mobile LTE" cleanly.
