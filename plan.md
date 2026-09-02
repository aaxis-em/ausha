# Ausha — Mobile Receiver Design Plan

Design plan for the network-side mobile receiver, plus the sender-side protocol
changes it depends on.

---

## 1. Where the project stands

The current sender (`player/`) does:

```
pactl (find RUNNING monitor sink)
  -> ffmpeg -f pulse -i <sink> -c:a aac -b:a 128k -ar 48000 -ac 2 -f mpegts -
    -> read 1400-byte chunks from stdout
      -> UdpSocket::send_to() to each registered client
```

Clients register by opening a TCP connection to port 6996; the server takes the
peer IP and assumes the receive port is 1234.

This is a good skeleton, but three properties of it directly constrain what the
mobile receiver can do, so they get fixed first.

### 1.1 Datagram size breaks MPEG-TS alignment

`conn/server.rs:8` uses a 1400-byte buffer. MPEG-TS packets are 188 bytes, and
1400 / 188 = 7.44. Every datagram straddles a TS packet boundary, so a single
lost datagram corrupts two TS packets and forces the receiver to hunt for the
next 0x47 sync byte. The conventional MPEG-TS-over-UDP payload is **1316 bytes**
(188 x 7), which fits inside a 1500-byte Ethernet MTU with IP+UDP headers.

If we stay on MPEG-TS, this is a one-line fix. We are not staying on MPEG-TS
(see 2.1), but it is worth understanding why.

### 1.2 `read()` boundaries are not frame boundaries

`ffmpeg_stdout.read(&mut buf)` returns whatever the pipe has, not a whole
anything. The receiver therefore cannot treat a datagram as a unit of audio,
which means it cannot do per-frame loss concealment. It can only resync the
container. That is the core reason the current transport caps how good the
receiver can be.

### 1.3 Client list is append-only

`create_connection_tcp` pushes `SocketAddr::new(addr.ip(), 1234)` and never
removes it. The vec grows without bound, we keep sending to phones that have
left, and the hardcoded 1234 breaks the moment two receivers sit behind the
same NAT or a phone cannot bind that port.

---

## 2. Protocol decisions

These are the decisions the receiver is built against. Each one is a
recommendation with the reasoning, not a menu.

### 2.1 Codec: Opus, not AAC

**Recommendation: Opus, 48 kHz, stereo, 20 ms frames, 96–160 kbps.**

| | AAC-LC in MPEG-TS (current) | Opus in RTP |
|---|---|---|
| Encoder lookahead | ~2048 samples (~43 ms) | ~6.5 ms |
| Frame size | 1024 samples fixed | 2.5–60 ms, selectable |
| Loss concealment | none | built-in PLC |
| Forward error correction | none | in-band LBRR FEC |
| Container overhead | 188-byte TS cells + PAT/PMT | 12-byte RTP header |
| Android decoder | MediaCodec (API 16+) | MediaCodec (API 21+) or libopus |

Opus was designed for exactly this job. The in-band FEC matters most: with
`-fec 1` and a declared expected packet loss, each packet carries a
low-bitrate copy of the *previous* frame, so an isolated loss is recovered
rather than concealed. On WiFi, isolated losses are the common case.

The one thing AAC-in-TS buys is that VLC opens the stream with no setup. We
keep that property a different way — see 2.2.

### 2.2 Framing: RTP, not a custom header

**Recommendation: standard RTP (RFC 3550) carrying Opus (RFC 7587).**

A custom header is tempting and would take an afternoon. RTP is the better call
because:

- It gives us exactly the fields the receiver needs — 16-bit sequence number,
  32-bit timestamp in 48 kHz sample units, 32-bit SSRC — in 12 bytes.
- One Opus frame per RTP packet. A datagram is now a self-contained unit of
  audio, which is what makes per-frame PLC and FEC possible.
- **Debuggability**: `ffplay -protocol_whitelist file,rtp,udp -i stream.sdp`
  plays it. Wireshark decodes it natively and will show you jitter and loss
  without you writing any tooling. When the Android audio glitches, being able
  to point ffplay at the same stream to bisect sender-vs-receiver is worth
  more than the bytes saved.

Packet layout:

```
 0                   1                   2                   3
 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|V=2|P|X|  CC   |M|     PT      |       sequence number         |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                timestamp (48 kHz sample clock)                |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                             SSRC                              |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                    Opus frame (one 20 ms frame)               |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
```

Payload type: 96 (dynamic). Timestamp advances 960 per 20 ms frame.
Typical packet is ~12 + 300 bytes at 128 kbps — far under MTU, no fragmentation.

### 2.3 Unicast, not multicast

Multicast looks right for "one desktop, several phones" but is a trap on WiFi:
access points transmit multicast at the lowest basic rate with no per-client
acknowledgement or retry, so loss is dramatically worse than unicast, and
Android needs a `MulticastLock` held to even receive it.

**Recommendation: unicast per client.** At 128 kbps, eight listeners is 1 Mbps
of upstream — nothing. Revisit only if the listener count goes into the dozens.

### 2.4 Control channel: TCP session, not a registration ping

Keep TCP:6996, but make it a real session. Newline-delimited JSON is fine and
keeps it inspectable with `nc`.

```
C->S  {"t":"hello","ver":1,"name":"Pixel 8","codecs":["opus"],"token":"<pairing>"}
S->C  {"t":"accept","ssrc":123456789,"codec":"opus","rate":48000,
       "ch":2,"ptime":20,"udp_port":<server's punch listener>}
C->S  (UDP punch packet from the client's receive socket)
S->C  {"t":"ready"}
...   {"t":"ping","ts":<sender monotonic us>} / {"t":"pong","ts":...,"rx":...}  every 2 s
S->C  {"t":"params","bitrate":128000,"fec":true}      # on change
C->S  {"t":"stats","loss":0.004,"jitter_ms":7,"buffer_ms":48}  every 5 s
C->S  {"t":"bye"}
```

Two things to note:

- **The server learns the client's UDP address from the punch packet**, not by
  assuming port 1234. This is the fix for 1.3 and it also survives NAT.
- **TCP disconnect is the removal signal.** The client entry lives with the
  connection handler; when `read()` returns 0 or errors, the handler removes it.
  This turns the append-only vec into a correct registry, and the periodic ping
  detects a phone that dropped off WiFi without a clean close.

### 2.5 Clock sync and drift

Two separate problems, often conflated:

**Offset** (what time is it on the sender?) — solved by the ping/pong above.
Client computes `offset = ((t1-t0) + (t2-t3)) / 2` NTP-style, keeps a rolling
minimum-RTT estimate. Needed only if you ever want A/V sync with video on the
desktop. Nice to have, not v1-critical.

**Drift** (the phone's DAC clock is not the sender's clock) — this one is
unavoidable and *will* bite. Crystal tolerance is tens of ppm; at 30 ppm the
buffer moves ~108 ms per hour, so the stream either underruns or accumulates
delay within an hour.

**Recommendation: buffer-level-driven micro-resampling.** A slow controller
watches the jitter buffer fill level over ~30 s windows and adjusts the
resampler ratio by at most +/-0.5%. Inaudible, and it converges. Do this in the
shared core, not on the platform side.

Rejected: `AudioTrack.setPlaybackParams(speed)` — it routes through the Sonic
time-stretcher, which is more DSP than needed and behaves inconsistently at
ratios this close to 1.0. Also rejected: dropping/inserting whole frames —
audible as clicks.

### 2.6 Security

The stream is desktop audio, which can be a private call. LAN-only is not a
security boundary — coffee shop WiFi is a LAN.

**Recommendation for v1:** pairing code shown on the desktop, entered on the
phone (or scanned as a QR containing host+port+token). The token authenticates
the TCP handshake.

**Recommendation for v1.5:** derive a key from the pairing code and wrap the RTP
payload in ChaCha20-Poly1305, using SSRC+sequence as the nonce. Cheap on mobile
(ARM has no AES-NI equivalent guarantee; ChaCha is the right choice there) and
closes the passive-eavesdropper hole. Note this breaks the ffplay debug path,
so keep it a flag that can be turned off during development.

---

## 3. Receiver architecture

### 3.1 Shared Rust core

**Recommendation: put the protocol in a `ausha-core` Rust crate shared by the
desktop sender, a desktop test receiver, and both mobile apps.**

The sender is already Rust. The hard parts of the receiver — RTP parsing,
reorder buffer, adaptive jitter estimation, FEC/PLC decisions, drift control —
are pure logic with no I/O and no platform dependency. Writing them once, in
the language the project already uses, with unit tests that inject synthetic
loss and jitter, is much better than writing them twice in Kotlin and Swift and
debugging them on a phone.

```
ausha-core/          # no I/O, no threads, no platform APIs
  rtp.rs             # parse/serialize RTP + Opus payload
  jitter.rs          # reorder buffer, adaptive target depth
  clock.rs           # offset estimation, drift controller
  session.rs         # control-channel state machine
  decode.rs          # libopus binding, PLC + FEC paths

ausha-desktop/       # existing sender + a CLI receiver (cpal output)
ausha-mobile-ffi/    # UniFFI bindings -> Kotlin + Swift
```

The core is a state machine driven by `on_packet(bytes, arrival_instant)` and
`fill_output(&mut [f32])`. It never blocks, never allocates on the audio path,
and never touches a socket. Platform code owns the socket and the audio device
and calls into it.

The cost is NDK/toolchain setup. That is a real cost, but it is a one-time
setup cost, whereas divergent Kotlin and Swift jitter buffers are a permanent
debugging cost.

### 3.2 Pipeline

```
UDP socket (platform)
  -> core.on_packet()          [network thread]
       parse RTP, drop dupes, reorder by sequence
  -> jitter buffer             [adaptive depth, see 3.3]
  -> audio callback            [realtime thread, must not block/allocate]
       core.fill_output():
         frame present  -> opus_decode()
         frame missing, next frame has FEC -> opus_decode(fec=1)
         frame missing, no FEC -> opus_decode(NULL)  # PLC
         drift resampler
  -> AudioTrack / AAudio (Android) or AVAudioEngine (iOS)
```

The realtime constraint is absolute: the audio callback must not lock a mutex
the network thread holds, must not allocate, and must not log. Use a
preallocated SPSC ring buffer between the network thread and the audio thread.

### 3.3 Adaptive jitter buffer

Fixed buffers are either too laggy on bad WiFi or too fragile on good WiFi.

- Track inter-arrival jitter (RFC 3550 smoothed estimate) over a sliding window.
- Target depth = p99 of recent jitter, clamped to **[20 ms, 200 ms]**.
- Grow immediately on underrun; shrink slowly (only after ~10 s of clean
  reception) so the buffer does not oscillate.
- Expose the target as a user-facing "Latency / Stability" slider with three
  presets — Low (20–40 ms), Balanced (40–80 ms), Stable (80–200 ms) — because
  the right answer genuinely depends on whether the user is watching video or
  listening to music in another room.

### 3.4 Latency budget

Target end-to-end for the Balanced preset:

| Stage | Budget |
|---|---|
| PulseAudio capture | 10–20 ms |
| Opus encode (20 ms frame + 6.5 ms lookahead) | ~27 ms |
| LAN transit | 1–5 ms |
| Jitter buffer | 40–80 ms |
| Opus decode | ~1 ms |
| Android output (AudioTrack low-latency) | 20–40 ms |
| **Total** | **~100–170 ms** |

**Call this out in the UI:** Bluetooth headphones add another 100–200 ms that
the app cannot control (SBC/AAC codec latency in the headset). A user on
Bluetooth will see ~300 ms no matter what we do, and will file it as our bug.
Wired or USB-C output is the only way to hit the numbers above.

---

## 4. Android app

Android first — it is the more open platform and the NDK path is better trodden.

### 4.1 Structure

```
app/
  ui/            Compose: discovery list, connect, volume, latency preset, stats
  service/       AushaService: foreground service, owns socket + audio thread
  native/        JNI/UniFFI bridge to ausha-core
```

### 4.2 Audio output

**v1: `AudioTrack` from Kotlin.**

- `AudioAttributes`: `USAGE_MEDIA`, `CONTENT_TYPE_MUSIC`
- `AudioFormat`: `ENCODING_PCM_FLOAT`, 48000, stereo
- `setPerformanceMode(PERFORMANCE_MODE_LOW_LATENCY)`
- Buffer = `getMinBufferSize()`, not a multiple of it — the instinct to
  oversize the buffer directly adds latency
- `MODE_STREAM`, blocking writes from a dedicated thread with
  `Process.setThreadPriority(THREAD_PRIORITY_URGENT_AUDIO)`

**v2 if v1 latency is unsatisfying: AAudio/Oboe via the NDK.** Since the core is
already native this is a smaller step than it sounds, and it removes a JNI
crossing from the audio path. Do not start here — measure v1 first.

### 4.3 The platform details that actually cause bugs

These are the ones that get discovered late and painfully:

- **WiFi power save.** This is the big one. Idle WiFi radios enter power save
  and add 100–200 ms latency spikes that look exactly like network jitter.
  Hold a `WifiLock` with `WIFI_MODE_FULL_LOW_LATENCY` (API 29+) while streaming.
  Without this the adaptive jitter buffer will grow to hide a problem that is
  entirely self-inflicted.
- **Foreground service.** Required to keep the socket alive when the screen is
  off. Android 14+ needs `foregroundServiceType="mediaPlayback"` in the manifest
  and the `FOREGROUND_SERVICE_MEDIA_PLAYBACK` permission.
- **Doze / battery optimization.** Even with a foreground service, aggressive
  OEM battery managers (Xiaomi, Samsung, OnePlus) kill background sockets.
  Detect and prompt for the battery-optimization exemption.
- **Audio focus.** Request `AUDIOFOCUS_GAIN`; duck or pause on an incoming call
  and resume after. Register a `MediaSession` so lock-screen and headset
  controls work.
- **Network change.** WiFi -> mobile data, or AP roaming, changes the local
  address. Register a `ConnectivityManager.NetworkCallback` and re-run the
  handshake rather than sitting on a dead socket.
- **Permissions.** `INTERNET`, `ACCESS_NETWORK_STATE`, `FOREGROUND_SERVICE`,
  `FOREGROUND_SERVICE_MEDIA_PLAYBACK`, `WAKE_LOCK`, `CHANGE_WIFI_MULTICAST_STATE`
  (mDNS needs this), and `POST_NOTIFICATIONS` for the service notification.

### 4.4 Discovery

**Recommendation: mDNS / DNS-SD.** Sender advertises `_ausha._tcp.local` on
port 6996 with TXT records for version and codec. Android side uses
`NsdManager`. Rust side uses the `mdns-sd` crate.

Always keep a manual "enter IP address" fallback — mDNS is unreliable across
VLANs, on enterprise WiFi with client isolation, and on some routers. A QR code
on the desktop encoding `ausha://host:port?token=...` is the nicest path and
also solves pairing-token entry.

---

## 5. iOS

Same core, different shell. Deferred until Android is solid.

- `AVAudioSession` category `.playback`, mode `.default`,
  `setPreferredIOBufferDuration(0.005)`
- `AVAudioSourceNode` (iOS 13+) pulling from the core, or a `RemoteIO`
  AudioUnit for the lowest latency
- `UIBackgroundModes: [audio]` — while audio is actively playing, the UDP
  socket stays alive. Note that if playback stops, iOS suspends the app and the
  socket dies; the reconnect path must handle this.
- `NWBrowser` for Bonjour discovery; add `NSLocalNetworkUsageDescription` and
  the `NSBonjourServices` key, or discovery silently returns nothing on iOS 14+
- Core builds as a static lib for `aarch64-apple-ios` and
  `aarch64-apple-ios-sim`; UniFFI generates the Swift bindings

---

## 6. Phasing

Ordered so that each phase is independently testable and the risky parts get
validated before any mobile code exists.

**Phase 0 — Sender protocol rework — DONE**
- [x] ffmpeg output switched to Opus, 20 ms frames, in-band FEC (`-fec:a 1` with
      `-packet_loss 5`; a bare `-fec` is swallowed by the RTP muxer)
- [x] RTP packetization, one frame per datagram, verified at 50 packets/s
- [x] Client registry rewritten: per-connection handler, UDP punch to learn the
      address, removal tied to the TCP connection
- [x] Session handshake, pairing token, and 2 s keepalive on TCP:6996
- [x] ffmpeg tied to the sender's lifetime (`Drop` + `PR_SET_PDEATHSIG`) after
      testing showed `kill <pid>` orphaned it holding the capture device
- *Done:* `ffplay` plays the stream from the SDP written by `--sdp-out`, and two
  mock receivers took the same SSRC and sequence range with zero gaps

See `arch.md` for the resulting design.

**Phase 1 — `ausha-core` + desktop receiver — DONE**
- [x] `ausha-core` extracted: RTP parse and sequence extension, jitter buffer,
      FEC/PLC decisions, drift controller, resampler, pipeline. No I/O.
- [x] `ausha-recv` desktop receiver; `player` now shares the core's protocol
- [x] 38 tests injecting 1/5/10% loss, reordering, 50 ms jitter bursts, and
      simulated hours of clock drift
- [x] Against a real sender at 10% random loss: 2666 packets, 333 lost, 285
      recovered by FEC, 48 concealed, **0 underruns, 0 silent frames**
- [x] 7-minute soak: depth held at target, 0 underruns, 0 loss

Four things this phase found that were not obvious from the plan:

1. **Loss bursts, not jitter, set the depth.** A run of N consecutive losses
   drains N frames without replacing any, so the target has to track the
   longest recent burst. Sizing from the jitter estimate alone underran.
2. **The drift controller must regulate undecoded depth**, not total buffered
   audio — decoded samples queued for the device cannot absorb a burst — and
   must regulate the depth left *after* a fill, not before it.
3. **Its gain had to be sized for recovery, not for drift.** At drift scale a
   20 ms shortfall took 100 s to close, longer than the gap between bursts.
4. **Resuming shallow after an underrun is worse than re-prebuffering.** It
   traded one 130 ms gap for five smaller ones and drove the target to its
   ceiling; growing the target *before* the underrun is the fix.

Deviation from the plan: the receiver's audio sink shells out to
`pacat`/`aplay`/`ffplay` rather than using `cpal`, because cpal needs ALSA
development headers this machine does not have. The sink is a two-method trait,
so a cpal backend drops in without touching the pipeline.

**Phase 2 — Android MVP — DONE**
- [x] NDK toolchain and `cargo-ndk` wired into the Gradle build, so the `.so`
      can never be stale relative to the Kotlin that calls it
- [x] `ausha-client` crate extracted, so the phone and the desktop receiver
      share the handshake and the pipeline rather than reimplementing either
- [x] Hand-written JNI (`ausha-mobile`) instead of UniFFI — see below
- [x] Foreground service, `AudioTrack` output, manual entry
- [x] *Verified on an emulator against the live sender:* handshake completed,
      `Playing`, 50 s at 0.00% loss, 0 concealed, 0 underruns, latency 75 ms,
      service `isForeground=true` with type `mediaPlayback`

**Phase 3 — Android production — DONE**
- [x] mDNS discovery (`_ausha._tcp`) on both ends, plus QR pairing:
      `ausha://host:port?token=…`, printed by `ausha --qr` and parsed from a
      scan or an opened link
- [x] WifiLock (`WIFI_MODE_FULL_LOW_LATENCY`), partial wake lock, audio focus
      with duck/pause, network-change reconnect
- [x] Stats screen: latency, buffer depth against target, jitter, loss, FEC
      recoveries, concealments, underruns, clock correction
- [x] Latency presets Low / Balanced / Stable, plumbed from the Compose UI
      through JNI into the jitter buffer's depth range
- [ ] MediaSession for lock-screen and headset controls — **not done**. The
      notification carries a Stop action and audio focus is handled, but there
      is no `MediaSession`, so headset buttons do nothing.

Deviations and findings:

1. **Hand-written JNI, not UniFFI.** The surface is five calls; UniFFI would
   have added a codegen step to the build for no benefit at this size.
2. **`ausha-client` is a new layer the plan did not have.** Core stays
   I/O-free as designed, but the sockets, threads and session handling needed
   a home that both the desktop receiver and the phone could share. Without it
   the handshake would have been written twice.
3. **mDNS advertised loopback.** Left to enumerate interfaces itself the
   daemon published 127.0.0.1, which resolves perfectly from the sending
   machine and is useless to a phone. The address now comes from the routing
   table.
4. **`collectAsStateWithLifecycle` crashed at startup** — lifecycle 2.8 wants a
   `LocalLifecycleOwner` this Compose version does not provide. Dropped for
   plain `collectAsState` rather than pinning a fragile version pair.

**Phase 4 — Hardening**
- [ ] MediaSession (carried over from Phase 3)
- ChaCha20-Poly1305 payload encryption behind a flag
- Multi-client soak test
- Windows sender (`capture/source.rs` returns an error on Windows)

**Phase 5 — iOS**

---

## 7. Open questions

1. **Is A/V sync with desktop video a goal?** If the user watches a film on the
   desktop and listens on the phone, we need the clock-offset work in 2.5 and a
   user-adjustable sync offset. If it is music-only, offset estimation can be
   dropped and only drift control is needed. This changes Phase 1's scope.
2. **How many simultaneous listeners?** Under ~8 the unicast decision in 2.3
   holds unconditionally. Above that, revisit.
3. **Does the phone ever need to send audio back?** If duplex is ever wanted,
   the session protocol in 2.4 should reserve room for it now rather than being
   retrofitted.
4. **Minimum Android API?** API 26 covers ~95% of devices and gives
   `PERFORMANCE_MODE_LOW_LATENCY` and float PCM. API 29 is needed for
   `WIFI_MODE_FULL_LOW_LATENCY`, which we can feature-detect.
