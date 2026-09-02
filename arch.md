<img src="assets/ausha@2x.png" alt="" width="60" align="right">

# Ausha Architecture

How the sender, the shared core, and the desktop receiver work today
(Phases 0 and 1 of `plan.md`). The mobile apps do not exist yet; they will use
the same `ausha-core` crate the desktop receiver uses.

---

## 0. Workspace

| Crate | Path | What it is |
|---|---|---|
| `ausha-core` | `core/` | Protocol types and the receive pipeline. No sockets, no threads, no platform APIs. |
| `ausha-client` | `client/` | Sockets, session and threads. Everything a receiver needs except the audio device. |
| `ausha-mobile` | `mobile/` | JNI bridge, built as a `.so` for each Android ABI. |
| `player` | `player/` | The sender, binary `ausha`. |
| `ausha-receiver` | `receiver/` | The desktop receiver, binary `ausha-recv`. |
| — | `android/` | The Android app. |

The layering is what stops the hard parts being written twice:

```
ausha-core     pure logic, no I/O          ← tested headlessly
   ↑
ausha-client   sockets, threads, session   ← shared by desktop and phone
   ↑                       ↑
ausha-recv             ausha-mobile → android/
(desktop sink)         (JNI)          (AudioTrack)
```

The desktop receiver and the Android app differ only in where samples end up.

`ausha-core` holds everything both ends agree on and everything that is hard to
get right — RTP parsing, the jitter buffer, FEC and concealment decisions,
drift correction. It is written once and tested headlessly, so the mobile apps
inherit a pipeline that already works rather than reimplementing it in Kotlin
and Swift.

> `player/` contains the sender, not the player. The name predates the split
> and is worth renaming.

---

## 1. Overview

Ausha captures whatever the desktop is playing and streams it to receivers on
the local network as Opus audio in RTP.

```
                      ausha (one process)
  ┌──────────────────────────────────────────────────────────────┐
  │                                                              │
  │  ffmpeg child                                                │
  │  ┌────────────────────────────┐                              │
  │  │ PulseAudio monitor source  │                              │
  │  │          ↓                 │                              │
  │  │ Opus encode, 20 ms frames  │                              │
  │  │          ↓                 │                              │
  │  │ RTP packetize              │                              │
  │  └────────────┬───────────────┘                              │
  │               │ UDP 127.0.0.1:5004  (one RTP packet/datagram) │
  │               ↓                                              │
  │        ┌─────────────┐        ┌──────────────┐               │
  │        │   relay     │◀──────▶│   registry   │               │
  │        └──────┬──────┘        └──────▲───────┘               │
  │               │                      │                       │
  │               │               ┌──────┴───────┐               │
  │               │               │   control    │ TCP  :6996    │
  │               │               └──────────────┘               │
  └───────────────┼──────────────────────────────────────────────┘
                  │ UDP :6997 → each receiver
                  ↓
        ┌─────────────────┬─────────────────┐
        │   phone A       │   phone B       │   ffplay (--static-client)
        └─────────────────┴─────────────────┘
```

Two channels, deliberately separated:

- **Control — TCP 6996.** Newline-delimited JSON. Handshake, pairing, stream
  parameters, keepalive. Reliable and ordered, because losing a handshake
  message is fatal while losing an audio frame is not.
- **Media — UDP 6997.** RTP carrying Opus. Unreliable on purpose: a late audio
  frame is worthless, so retransmission would only add latency.

---

## 2. Why these choices

### Opus, not AAC

The original prototype encoded AAC into an MPEG-TS container. Three problems
made it unsuitable for a real-time receiver:

- AAC-LC carries ~2048 samples (~43 ms) of encoder lookahead. Opus carries
  ~6.5 ms.
- MPEG-TS has no loss recovery. Opus has in-band FEC and packet-loss
  concealment, which is what makes audio over WiFi survivable.
- TS packets are 188 bytes and the prototype read 1400-byte chunks
  (1400 / 188 = 7.44), so every datagram straddled a TS packet boundary. One
  lost datagram corrupted two TS packets and forced a resync.

### RTP, not a custom header

RTP costs 12 bytes and provides the three fields a receiver actually needs — a
16-bit sequence number, a 32-bit timestamp on the 48 kHz sample clock, and an
SSRC identifying the stream. It also keeps the stream playable by standard
tools, which is why `--sdp-out` exists: `ffplay` can stand in for the mobile
app during development.

### One Opus frame per datagram

ffmpeg's RTP muxer emits exactly one 20 ms Opus frame per packet. This is the
property the whole receiver design rests on: a datagram is a self-contained
unit of audio, so a loss costs exactly one frame and the receiver can conceal
it. Chunked byte streams cannot do this.

### An MPEG-TS side output stays available

RTP with a dynamic payload type cannot be played without an SDP file, which is
awkward to get onto a phone. MPEG-TS is self-describing and resyncs on its own,
so `mpv udp://0.0.0.0:1234` just works with nothing to copy across.

`--compat-ts <ip:port>` therefore adds a second ffmpeg output muxing the same
audio to MPEG-TS. It exists so a phone running mpv or VLC can listen before the
native receiver is built. It is a convenience path, not the design target:

- It bypasses the handshake, so it bypasses the pairing token too.
- MPEG-TS has no loss recovery, so the receiver cannot conceal a lost frame.
- `-pkt_size 1316` is seven 188-byte TS packets, which is the alignment the
  original prototype got wrong.

The stream still carries Opus, because it decodes identically to the RTP path
and avoids AAC's lookahead. mpv logs `Error parsing Ogg TS header` while
probing Opus-in-TS; it is cosmetic, and decoding was verified byte-exact.

### Unicast, not multicast

WiFi access points transmit multicast at the lowest basic rate with no
per-client acknowledgement or retry, so multicast loss is far worse than
unicast. At 128 kbps, eight receivers cost 1 Mbps of upstream — not worth
optimising.

---

## 3. Modules

All under `player/src/`.

| Module | Responsibility |
|---|---|
| `main.rs` | Binds sockets, wires the threads together, runs the forwarding loop |
| `cli.rs` | Command line arguments |
| `config.rs` | Stream and protocol constants |
| `capture/mod.rs` | Builds the ffmpeg command line and owns the child process |
| `capture/source.rs` | Finds the platform's loopback audio device |
| `control.rs` | TCP control server: handshake, auth, keepalive |
| `protocol.rs` | Control message types |
| `lines.rs` | Newline framing that tolerates read timeouts |
| `registry.rs` | Which receivers exist and where to send their media |
| `relay.rs` | Punch listener, RTP fan-out, MPEG-TS compat fan-out |
| `sdp.rs` | Session description for the ffplay debug path |
| `ids.rs` | Session ids, SSRC, pairing token |

### Threads

| Thread | Work |
|---|---|
| main | Reads RTP from the ingest socket, forwards to every receiver |
| control accept | Accepts TCP connections, spawns one thread per session |
| control session (per receiver) | Handshake, then keepalive until disconnect |
| punch listener | Reads the media socket, records receiver addresses |
| compat fan-out (only with `--compat-ts`) | Forwards MPEG-TS to fixed targets |

Shared state is one `Registry` behind a `Mutex`. The fan-out holds that lock
while looping over receivers, which is acceptable because it runs 50 times a
second over a handful of non-blocking `send_to` calls.

---

## 4. Session lifecycle

```
receiver                                             sender
   │                                                    │
   │──── TCP connect :6996 ─────────────────────────────▶│
   │──── {"t":"hello","ver":1,"name":..,"token":..} ────▶│
   │                                                    │  verify token
   │◀─── {"t":"accept","session":"<16 hex>",             │  create session
   │      "media_port":6997,"stream":{...}}              │
   │                                                    │
   │──── UDP "AUSHA/1 <session>" → :6997 ───────────────▶│  learn address
   │◀─── {"t":"ready"} ─────────────────────────────────│
   │                                                    │
   │◀═══ RTP/Opus, 50 packets/second ═══════════════════│
   │                                                    │
   │◀─── {"t":"ping","ts":<µs>}  every 2 s ─────────────│
   │──── {"t":"pong","ts":<echo>} ─────────────────────▶│
   │                                                    │
   │──── {"t":"bye"} or TCP close ─────────────────────▶│  remove session
```

### Why the UDP punch

The prototype assumed each receiver listened on port 1234 and derived the
address from the TCP peer IP. That breaks as soon as two receivers sit behind
one NAT, or a phone cannot bind that port.

Instead the receiver sends one UDP datagram naming its session id, and the
sender records the source address it actually arrived from. This works through
NAT, needs no fixed port, and — because the sender replies from the same
socket the punch arrived on — keeps any NAT mapping open.

The control thread blocks on the punch for up to 10 s, then sends `ready`.

### Why the sender pings

TCP will not notice a phone that left WiFi without closing cleanly, sometimes
for many minutes, and the registry would keep sending audio to nobody. A ping
every 2 s with a 10 s liveness deadline bounds that.

The ping carries the sender's wall clock in microseconds. Nothing uses it yet;
it is the hook for the receiver-side clock offset estimation described in
`plan.md` §2.5.

### Removal is tied to the connection

Each session lives with its TCP connection handler. When that handler returns —
clean `bye`, socket close, keepalive timeout, or error — it removes the session
from the registry. There is no separate reaper and no way for an entry to
outlive its connection.

---

## 5. The receive pipeline (`ausha-core`)

Datagrams go in, playable audio comes out. The caller owns the socket and the
audio device and supplies the clock:

```rust
pipeline.on_datagram(&datagram, arrival_us);   // network thread
pipeline.fill(&mut output, now_us);            // audio thread
```

```
datagram
   │  rtp::parse             version, CSRC, extension, padding
   ↓
jitter::JitterBuffer         reorder by extended sequence, adaptive depth
   │
   ├─ Step::Decode(payload)  the frame arrived
   ├─ Step::Recover(next)    lost, but the next packet carries a copy
   ├─ Step::Conceal          lost with no FEC source
   └─ Step::Starve           not enough buffered yet
   ↓
decode::Decoder              libopus: decode / decode-with-FEC / PLC
   ↓
resample::Resampler          Catmull-Rom, ratio from the drift controller
   ↓
ready queue ──▶ output buffer
```

### Sequence numbers

RTP sequence numbers are 16 bits and wrap every 65536 packets, about every 22
minutes at 50 packets per second. `rtp::SequenceExtender` lifts them into a
monotonic 64-bit space, handling the case where a packet reordered *across* the
wrap must not be treated as 65535 packets in the future.

### What the jitter buffer decides

For every output slot it answers one question: what should the decoder do?

- The frame is present — decode it.
- The frame is missing but the *next* one has arrived — decode that one with
  Opus in-band FEC, which reconstructs the frame before it. This is why the
  buffer holds a frame of lookahead: the repair data arrives after the damage.
- The frame is missing and nothing follows it yet — run packet loss
  concealment, which extrapolates from decoder state.
- Nothing is buffered — play silence and refill.

Frames arriving before `next` are counted late and dropped rather than played
out of order. Duplicates are counted and ignored.

### Choosing the depth

The target depth is the largest of two demands, clamped to 40–200 ms:

- **Jitter.** Three times the RFC 3550 interarrival jitter estimate, plus a
  frame.
- **Loss bursts.** Two frames more than the longest run of consecutive losses
  seen recently.

The burst term is the one that matters in practice. A run of N consecutive
losses drains N frames of depth without replacing any of them, so surviving it
needs a buffer deeper than N. Reacting to observed bursts raises the target
*before* a long run causes a dropout, rather than after. Measured against a
real sender at 10% loss, the target settles around 135 ms.

The target grows immediately and shrinks by 5 ms steps only after 10 s without
growing, so it does not oscillate on a noisy link. An underrun also grows it,
since running dry is proof the estimate was too low.

### Drift correction

The sender's and the receiver's clocks are different crystals. Thirty parts per
million — ordinary — moves the buffer 108 ms in an hour, so an uncorrected
stream eventually either underruns or accumulates delay.

`drift::DriftController` watches the buffer depth once a second and returns a
resampling ratio within ±0.5% of nominal, with a 5 ms deadband so it tracks
drift rather than jitter. `resample::Resampler` applies it with Catmull-Rom
interpolation.

Two details that took measurement to get right:

- **It regulates undecoded depth, not total buffered audio.** Samples already
  decoded and queued for the device are on their way out and cannot absorb a
  loss burst. Counting them let the buffer sit permanently one chunk short of
  its target.
- **It regulates the depth left *after* a fill has taken its frames**, not
  before. The post-fill trough is the depth a burst actually has to survive;
  regulating the pre-fill peak left the real floor a frame lower than intended.

The gain is sized for recovery rather than for drift. When the target grows,
the depth has to follow within seconds; at the original drift-scale gain a
20 ms shortfall would have taken 100 seconds to close, which is longer than the
gap between bursts.

---

## 6. The desktop receiver (`ausha-recv`)

A thin shell around the core: it owns the sockets, the threads, and the audio
sink.

| Thread | Work |
|---|---|
| main | Drains arrivals into the pipeline, fills a chunk, writes it to the sink |
| receive | Blocks on the UDP socket, hands datagrams over a channel |
| control | Answers pings, reports reception stats |

Datagrams reach the playback thread through an `mpsc` channel rather than a
shared lock. That is not an optimisation for the desktop — it is the shape a
real audio callback needs, so the mobile apps can keep the same structure.

The control channel lives on its own thread because it blocks for up to half a
second at a time. Putting it in the playback loop stalled audio for as long as
the gap between pings.

### Audio output

The sink is a child process reading raw `f32` samples on stdin — `pacat`,
`aplay`, or `ffplay`, whichever is present. This avoids native audio build
dependencies, and blocking writes pace the pipeline in real time for free.
`--sink null` discards audio at real time for soak testing.

This is the one piece the mobile apps will not reuse. Everything upstream of it
already knows nothing about how audio reaches a speaker.

---

## 7. Android

```
PlaybackService (foreground)
   ├─ WifiLock, wake lock, audio focus, network callback
   └─ AudioEngine  ── thread "ausha-audio" ──┐
                                            ↓
                              Native.nativeFill(handle, FloatArray)
                                            ↓  JNI
                              ausha-client → ausha-core pipeline
                                            ↓
                                     AudioTrack (float, low latency)
```

Gradle runs `cargo-ndk` as a build task, so the `.so` cannot be stale relative
to the Kotlin that calls into it — a mismatched JNI signature is only found at
call time, which is a bad place to find it.

### What runs where

Only the audio device is Kotlin's. The handshake, jitter buffer, FEC, drift
correction and stats all run in the Rust core, reached through five JNI calls:
connect, fill, stats, isRunning, disconnect. The pull loop is the sole caller
of `nativeFill`; the UI reads stats, which the core guards separately.

Hand-written JNI rather than UniFFI: at five calls the codegen step would cost
more than it saves.

### Platform details that actually cause bugs

- **WiFi power save** is the biggest avoidable latency source. An idle radio
  parks between beacons and adds spikes indistinguishable from network jitter,
  which would make the buffer grow to hide a problem we caused. The service
  holds `WIFI_MODE_FULL_LOW_LATENCY`.
- **Foreground service** with `mediaPlayback` type, or the socket dies with the
  screen.
- **Audio focus** pauses for calls instead of talking over them.
- **Network changes** — roaming or dropping to mobile data changes the local
  address and strands the UDP socket silently, so a `NetworkCallback` triggers
  a reconnect.
- **Multicast lock** — without it the radio filters the multicast mDNS rides on
  and discovery silently returns nothing.
- **Bluetooth adds 100–200 ms** the app cannot control. Wired output is the
  only way to hit the latency figures above.

### Discovery and pairing

The sender advertises `_ausha._tcp` with TXT records for version, codec, rate,
channels and bitrate, and prints `ausha://host:port?token=…` — as a link, and
as a QR code with `--qr`. The app finds senders with `NsdManager`, and accepts
the same link from a scan or an opened URL.

Discovery is always a convenience, never the only way in: mDNS is blocked
across VLANs, on guest networks with client isolation, and on many consumer
routers, so manual entry stays.

The sender publishes the address from the routing table rather than letting the
daemon enumerate interfaces, which advertised loopback — resolvable from the
sending machine and useless to a phone.

---

## 8. Capture

`capture/source.rs` asks PulseAudio for the default sink and appends
`.monitor`, which is the canonical way to capture what the desktop is playing.
If that fails it falls back to the first source whose name ends in `.monitor`.

The generated ffmpeg command:

```
ffmpeg -hide_banner -loglevel warning -fflags nobuffer
       -f pulse -fragment_size 3840 -i <sink>.monitor
       -c:a libopus -b:a 128k -ar 48000 -ac 2
       -application audio -frame_duration 20
       -packet_loss 5 -fec:a 1
       -payload_type 96 -ssrc <random>
       -muxdelay 0 -muxpreload 0
       -map 0:a -f rtp rtp://127.0.0.1:5004?rtcpport=5005
```

With `--compat-ts`, a second output is appended so both streams come from one
capture and one process:

```
       -map 0:a -c:a libopus -b:a 128k
       -f mpegts udp://127.0.0.1:<port>?pkt_size=1316
```

Like the RTP stream it is muxed to loopback and fanned out in `relay.rs`, so
several devices cost one encode. The muxer logs `frame size not set` once at
startup; playback is unaffected.

Points worth knowing:

- **`-fec:a`, not `-fec`.** The RTP muxer has its own unrelated `fec` option
  and will consume a bare `-fec`, failing with "Unsupported FEC protocol 1".
  The `:a` stream specifier directs it to the audio encoder.
- **`-packet_loss 5` is required for FEC to do anything.** libopus only emits
  the redundant in-band copy when expected loss is non-zero.
- **`-fragment_size 3840`** is one 20 ms stereo s16 period at 48 kHz, which
  keeps PulseAudio from buffering more than one frame.
- **RTCP port.** RTP convention puts RTP on an even port and RTCP on the odd
  port above it. The sender binds both; the RTCP socket is never read, and
  exists only so ffmpeg's reports do not draw ICMP port-unreachable replies.

### ffmpeg cannot be orphaned

Two mechanisms, because either alone leaves a gap:

- `Encoder` implements `Drop`, killing and reaping the child on any normal
  return or panic.
- The child sets `PR_SET_PDEATHSIG` to `SIGKILL` before `exec`, so the kernel
  kills it even when the sender dies from a signal it cannot handle.

Without the second, `kill <sender-pid>` left ffmpeg running and holding the
capture device — observed in testing.

---

## 9. Security

The pairing token is a 48-bit random value, displayed grouped as
`xxxx-xxxx-xxxx`, generated fresh per run unless `--token` fixes it. A receiver
must present it in `hello`. Comparison is constant-time with respect to how
many leading characters match.

**The media stream itself is not yet encrypted.** Anyone on the network who can
capture packets can decode the audio. `plan.md` §2.6 covers adding
ChaCha20-Poly1305 to the RTP payload. Treat a LAN as untrusted until then.

---

## 10. Running it

```bash
cd player
cargo run
```

Prints the pairing token, control port, media port and SSRC.

### Without a mobile app

```bash
cargo run -- --static-client 127.0.0.1:5555 --sdp-out /tmp/ausha.sdp
ffplay -protocol_whitelist file,rtp,udp -i /tmp/ausha.sdp
```

`--static-client` adds a permanent media target that skips the handshake, and
`--sdp-out` writes the matching session description. Replace `127.0.0.1:5555`
with another machine's address to test across the network.

### With the desktop receiver

```bash
cargo run --release --bin ausha-recv -- --host <sender-ip> --token <token>
```

Useful flags: `--sink null` to run headless, `--run-for <seconds>` to bound a
soak, and `--simulate-loss <pct>` to drop received packets and exercise
concealment against a real sender.

---

## 11. Known limitations

- **Windows capture is unimplemented.** `capture/source.rs` returns an error.
  ffmpeg has no WASAPI loopback demuxer, so this needs a DirectShow loopback
  device such as virtual-audio-capturer. Tracked as Phase 4.
- **No discovery.** Receivers need the sender's IP. mDNS is Phase 3.
- **`--compat-ts` is unauthenticated.** It skips the handshake entirely, so it
  skips pairing. Use it only on a trusted network.
- **No encryption** on the media path. See §6.
- **No RTCP.** Receiver reports would give the sender real loss and jitter
  figures; today it learns them only from the control channel `stats` message,
  which nothing sends yet.
- **`player/` holds the sender, not the player.** The directory name predates
  the split between sender and receiver and is worth renaming.
- **No iOS app.** `ausha-core` and `ausha-client` are ready for it; only the
  audio sink and a Swift bridge remain.
- **No MediaSession**, so headset buttons and lock-screen transport controls do
  nothing. The notification carries a Stop action.
- **The clock offset is estimated but unused.** `Session::offset_us` tracks it
  from the ping timestamps; A/V sync with desktop video would consume it.
- **No cpal backend.** The sink shells out to `pacat`/`aplay`/`ffplay` rather
  than binding a native audio API, which costs some latency control.
