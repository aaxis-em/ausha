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
| `capture/dshow.rs` | Reads ffmpeg's DirectShow device list on Windows |
| `capture/lifetime.rs` | Ties ffmpeg's lifetime to the sender's, per platform |
| `uplink.rs` | Receives a client's microphone and decodes it at real time |
| `pipesource.rs` | Publishes that as a PulseAudio capture device |
| `shutdown.rs` | Turns SIGINT and SIGTERM into a flag the main loop acts on |
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
| uplink (only with `--uplink`, while a client holds it) | Decodes the microphone and meters it into the pipe |
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
   │      "media_port":6997,"stream":{...}}              │  (stream.encryption
   │                                                    │   names the salt)
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
   │  crypto::Opener         only with --encrypt; in ausha-client, not here
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

Decryption is the one step that happens outside the pipeline: it belongs to the
transport, and doing it in `ausha-client`'s receive thread means the pipeline
only ever sees plaintext RTP, whatever the transport did. `ausha-core` still
owns the cipher, in `crypto`, so both ends share one implementation.

### Sequence numbers

RTP sequence numbers are 16 bits and wrap every 65536 packets, about every 22
minutes at 50 packets per second. `rtp::SequenceExtender` lifts them into a
monotonic 64-bit space, handling the case where a packet reordered *across* the
wrap must not be treated as 65535 packets in the future.

It splits that into `peek` and `commit` so that a caller which can still reject
a packet extends the sequence number before deciding to trust it — which is
what keeps a forged packet from moving the encryption rollover counter.

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
   ├─ WifiLock, wake lock, audio focus, default-network callback
   ├─ Transport → MediaSession: headset keys, lock screen, media panel
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
  a reconnect. It watches the *default* network: a phone holding both WiFi and
  mobile data raises `onLost` for whichever it drops, and reconnecting on that
  cut playback on a device that had lost nothing.
- **A media button starts the service.** `MediaButtonReceiver` starts it as a
  foreground service to deliver a headset press, so it must call
  `startForeground` even when there is nothing to resume.
- **Multicast lock** — without it the radio filters the multicast mDNS rides on
  and discovery silently returns nothing.
- **Bluetooth adds 100–200 ms** the app cannot control. Wired output is the
  only way to hit the latency figures above.

### Call mode

Call mode is a mode, not a setting. The echo that would otherwise make this
unusable — we play the far end, the speaker feeds it into the microphone, the
far end hears itself — is cancelled by the platform, but only on the
communication audio path: `MODE_IN_COMMUNICATION`, capture from
`VOICE_COMMUNICATION`, playback with `USAGE_VOICE_COMMUNICATION`. The canceller
references whatever the device is playing, and there our downlink *is* what the
device is playing, so the reference signal is exactly what needs removing. That
is the hard part of echo cancellation, and the platform hands it over.

It is paid for in fidelity: that path does not honour
`PERFORMANCE_MODE_LOW_LATENCY` and commonly runs at 16 kHz. So turning call
mode on tears down and rebuilds `AudioTrack`, and both directions move to the
`Voice` buffer depth, which is tighter than any listening preset because a
conversation notices delay more readily than a concealed frame.

The microphone thread lives inside `AudioEngine` rather than beside it, because
the native handle does: anything touching the handle has to be gone before it
is freed, and one owner makes that orderable. Pausing from the lock screen
stops the microphone with everything else, which matters more than it sounds —
a paused call still transmitting the room is a privacy bug.

`RECORD_AUDIO` is asked for when call mode is switched on, not at startup, and
the service adds the `microphone` foreground type only while it holds the
permission, which API 30 requires before `startForeground` rather than merely
declared. A microphone foreground service also cannot be started from the
background on API 31+, which is why a pairing link never turns call mode on.

### Transport controls

`Transport` owns a `MediaSessionCompat`; the notification is a `MediaStyle`
notification pointing at it, and both the headset button and the system media
panel reach playback through its callbacks rather than through the service's
own actions.

A live stream has no seek, no skip and no duration, so the session offers only
play, pause and stop, and reports `PLAYBACK_POSITION_UNKNOWN` rather than a
position the system would draw as a progress bar that never moves. Pause means
leaving the sender — there is no backlog to resume from — so play reconnects.
Pausing releases audio focus and the WiFi and wake locks, and a deliberate
pause is remembered so that regaining focus or a network does not restart a
stream the listener stopped on purpose.

Unplugging headphones (`ACTION_AUDIO_BECOMING_NOISY`) pauses rather than
playing out loud.

### Discovery and pairing

The sender advertises `_ausha._tcp` with TXT records for version, codec, rate,
channels and bitrate, and prints `ausha://host:port?token=…` — as a link, and
as a QR code unless `--no-qr` says otherwise. The app finds senders with `NsdManager`, and accepts
the same link from a scan or an opened URL.

Discovery is always a convenience, never the only way in: mDNS is blocked
across VLANs, on guest networks with client isolation, and on many consumer
routers, so manual entry stays.

The sender publishes the address from the routing table rather than letting the
daemon enumerate interfaces, which advertised loopback — resolvable from the
sending machine and useless to a phone.

---

## 8. Duplex: the phone as a microphone

`--uplink` turns the sender into a capture device as well as a source, so a
call taken on the desktop can be spoken into from the phone. The downlink
already carries the call's audio to the phone — it comes out of the default
sink like anything else — so this is the other half.

```
phone microphone
   │  AudioRecord, VOICE_COMMUNICATION, mono 48 kHz
   ↓
encode::Encoder      Opus voip, 32 kbps, 20 ms, in-band FEC
   ↓
rtp::Builder         payload type 97, the sender's per-session SSRC
   ↓
crypto::Sealer       with --encrypt, under the uplink's own key
   ↓  UDP to the sender's uplink port
pipeline::Pipeline   the same jitter buffer, FEC and concealment, at Voice depth
   ↓
pipesource           s16le into a FIFO, metered out at real time
   ↓
module-pipe-source   a capture device named "ausha"
```

The receive half is the pipeline the phone already runs on the downlink, in the
other direction: it never knew which way the audio was going.

**One at a time.** Two phones feeding one microphone would need mixing and a
policy for whose voice wins. The second client is still accepted — as a
listener — because dropping someone's audio for that would be the worse trade.

**Demultiplexed by SSRC.** The sender assigns it per session and it rides in the
clear in the header, so it survives a NAT rebinding mid-call that a source
address would not.

**The device lives as long as the sender, not as long as the call.** Tying it
to the session looked tidier and was wrong: a call application that has already
selected it does not cope with it disappearing. When the phone dropped
mid-call the application silently fell back to the laptop's own microphone,
which is worse than silence because nobody notices. So `--uplink` publishes the
device at startup and it stays; a session claiming it only decides what comes
out. Idle, it produces silence — a muted microphone, which is what it is.

That leaves the module outliving the process that loaded it, and two things
handle it:

- A signal unwinds nothing, so `shutdown.rs` turns SIGINT and SIGTERM into a
  flag and `main` unloads whatever is still registered on the way out. Without
  it, killing the sender left a capture device in everyone's input list that
  nothing would remove.
- A run that was killed harder than that is swept up at the next start, by
  matching the pipe the stale module owns.

**The pipe is written non-blocking.** Nothing recording from the source lets it
back up, and blocking there would stall the uplink for as long as nobody was
listening. A frame is well under `PIPE_BUF`, so the write is all-or-nothing and
a dropped frame cannot shift every sample after it. The sender says so when it
starts discarding, because a working uplink that nobody selected looks
identical to a broken one.

**It needs its own clock.** Every other pipeline here is paced by an audio
device; a pipe takes whatever it is given as fast as it is given. Frames are
metered against the sample count rather than a timer, so a slow iteration is
made up instead of accumulating.

**Windows has no equivalent**, so this is Linux-only until someone wires up a
virtual cable driver.

---

## 9. Capture

`capture/source.rs` asks PulseAudio for the default sink and appends
`.monitor`, which is the canonical way to capture what the desktop is playing.
If that fails it falls back to the first source whose name ends in `.monitor`.

On Windows there is no equivalent — Windows has no monitor source and ffmpeg
has no WASAPI loopback demuxer — so `capture/dshow.rs` reads ffmpeg's
DirectShow device list and picks a loopback device from it, preferring the
`virtual-audio-capturer` filter (which works on any card) over a card's own
Stereo Mix (which only exists if the driver exposes it). Two listing layouts
are in the wild, one grouping devices under a heading and one tagging each line
`(audio)`; both are parsed, and that parsing is tested on Linux because it is
the same text whatever host produced it.

`--capture <device>` overrides detection on either platform, for a sound setup
only the user knows the shape of.

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
- The kernel is asked to kill the child when the sender dies, so it goes even
  when the sender dies from something it cannot handle. On Linux the child sets
  `PR_SET_PDEATHSIG` to `SIGKILL` before `exec`; Windows has no equivalent, so
  the child is put in a job object with `KILL_ON_JOB_CLOSE` after spawn, which
  the kernel empties when the last handle to it closes.

Without the second, `kill <sender-pid>` left ffmpeg running and holding the
capture device — observed in testing.

`capture/lifetime.rs` holds both, behind one `Guard` the encoder owns for as
long as it owns the child.

---

## 10. Security

The pairing token is a 48-bit random value, displayed grouped as
`xxxx-xxxx-xxxx`, generated fresh per run unless `--token` fixes it. A receiver
must present it in `hello`. Comparison is constant-time with respect to how
many leading characters match.

### Media encryption

`--encrypt` wraps the RTP payload in ChaCha20-Poly1305. It is off by default,
because turning it on is what stops `ffplay` and the SDP debug path working.

```
[ RTP header, 12 bytes, clear ][ ciphertext ][ Poly1305 tag, 16 bytes ]
   authenticated as AAD          everything past the header
```

The header stays clear the way SRTP leaves it clear: a capture still shows
sequence numbers, timestamps and SSRC, so loss and jitter stay diagnosable in
Wireshark, while the audio does not leave the machine in the clear. It is
authenticated as associated data, so the sequence number a packet claims cannot
be changed.

The key is PBKDF2-HMAC-SHA256 at 200k rounds over the pairing token, salted
with 16 random bytes the sender generates per run and names in `accept`. Two
things drive that:

- **The salt is per run**, so a pinned `--token` still yields a different key
  every time the sender starts.
- **The rounds are not decoration.** A 48-bit token derived cheaply is a few
  GPU-hours to brute force against a captured stream. At 200k rounds that is
  out of reach, and it costs one handshake a couple of hundred milliseconds.

Each direction takes its own key from that secret, split with a one-shot hash
over a direction label. They must not share one: the nonce is the SSRC and the
sequence number, and a downlink and an uplink packet can carry the same pair,
which under one key is a repeated nonce.

The nonce is the SSRC and the *extended* sequence number, which never repeat
within a session. The 16-bit sequence alone would not do: it wraps every 22
minutes at 50 packets per second, and a repeated nonce under one key is the one
failure ChaCha20-Poly1305 does not survive. The receiver's rollover counter
only advances on a packet that authenticates, so a forged sequence number
cannot walk it away from the sender's.

A packet that fails to authenticate is dropped without a word: to the jitter
buffer that is indistinguishable from a packet that never arrived, which it is
already equipped to conceal.

**What this does not cover.** The sender is not authenticated — the token
proves the receiver to the sender, not the reverse — so an active attacker in
the path can still impersonate a sender, and `accept` is where they would strip
the `encryption` field to force plaintext. This closes the passive
eavesdropper, which is the realistic threat on a shared network.

`--compat-ts` is never encrypted; it exists so that players with no pairing
step can receive the stream, and it skips the token as well.

`--encrypt` covers the session, so an uplink is encrypted whenever the downlink
is.

---

## 11. Running it

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

### Using the phone as a microphone

```bash
cargo run --release --bin ausha -- --uplink
```

The **ausha** input appears straight away and stays for as long as the sender
runs, so it can be selected in the call application before the phone is
anywhere near it. Switch call mode on in the app to start talking through it.
To check the path without a phone:

```bash
cargo run --release --bin ausha-recv -- --host <ip> --token <token> \
    --sink null --uplink-tone
parecord --device=ausha --format=s16le --rate=48000 --channels=1 tone.wav
```

A 440 Hz tone in `tone.wav` means every link between the two works.

### Soaking several receivers at once

```bash
./scripts/soak.sh -n 8 -d 240 -l 3 -e -u
```

Starts a sender and N receivers against it, then fails if any of them saw an
underrun or a silent frame — the two things a listener actually hears. Loss on
its own is not a failure; absorbing it is the receiver's job. The script also
prints the spread in packet counts across receivers, which is how a fan-out
that served one receiver better than another would show up. `-u` adds an
uplink, which also exercises the second receiver being refused the microphone.

This is the one test that needs a network. The core's own tests inject loss,
reordering and jitter headlessly; what they cannot exercise is one encoder, one
fan-out and one registry serving several sessions at once over a real socket.

---

## 12. Known limitations

- **The Windows sender is untested.** The code is there and type-checks for
  `x86_64-pc-windows-*`, but it has never been run on Windows: no machine to
  run it on. It also needs a loopback device the user installs or enables
  themselves, because Windows ships none.
- **`--compat-ts` is unauthenticated.** It skips the handshake entirely, so it
  skips pairing, and `--encrypt` does not apply to it. Use it only on a trusted
  network.
- **Encryption is opt-in and one-directional.** See §10: `--encrypt` closes the
  passive eavesdropper, not an active attacker in the path.
- **No RTCP.** Receiver reports would give the sender real loss and jitter
  figures; today it learns them only from the control channel `stats` message,
  which nothing sends yet.
- **`player/` holds the sender, not the player.** The directory name predates
  the split between sender and receiver and is worth renaming.
- **Call mode's echo cancellation is unverified.** The code is there and the
  transport is proven, but whether the platform canceller actually removes our
  downlink has never been tried on hardware with a real acoustic path. Until it
  is, treat call mode as safe with headphones on the phone and unproven on
  speaker.
- **The uplink is Linux-only**, for want of a virtual capture device elsewhere.
- **No iOS app.** `ausha-core` and `ausha-client` are ready for it; only the
  audio sink and a Swift bridge remain.
- **The clock offset is estimated but unused.** `Session::offset_us` tracks it
  from the ping timestamps; A/V sync with desktop video would consume it.
- **No cpal backend.** The sink shells out to `pacat`/`aplay`/`ffplay` rather
  than binding a native audio API, which costs some latency control.
