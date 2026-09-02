<p align="center">
  <img src="assets/ausha@2x.png" alt="" width="89">
</p>

<h1 align="center">Ausha</h1>

<p align="center">
  A real-time audio transportation mechanism — streams desktop audio to
  receivers on the local network as Opus over RTP.
</p>

<p align="center">
  <a href="https://aaxis-em.github.io/ausha/">Website</a>
</p>

- `arch.md` — how it works
- `plan.md` — roadmap, including the mobile receiver design
- `CLAUDE.md` — code style rules
- `index.html` — the project page. Serve it by setting GitHub Pages to deploy
  from `main`, folder `/ (root)`.

## Status

The sender captures desktop audio, encodes Opus in 20 ms frames, and fans the
RTP stream out to every paired receiver. The desktop receiver plays it back
with a jitter buffer, FEC recovery, loss concealment and clock-drift
correction. The Android app does the same on a phone, with mDNS discovery and
QR pairing. iOS is not built yet; it will reuse the same Rust crates.

## Layout

| Crate | Binary | What it is |
|---|---|---|
| `core/` | — | `ausha-core`: protocol and receive pipeline, no I/O |
| `player/` | `ausha` | The sender |
| `client/` | — | `ausha-client`: sockets, session and threads, no audio device |
| `mobile/` | — | `ausha-mobile`: the JNI bridge, built as a `.so` |
| `receiver/` | `ausha-recv` | The desktop receiver |
| `android/` | — | The Android app |

## Requirements

ffmpeg built with `libopus`, plus a loopback audio device.

| Platform | Capture | State |
|---|---|---|
| Linux | PulseAudio monitor source | working |
| Windows | DirectShow loopback device | not implemented |

The receiver additionally needs one of `pacat`, `aplay` or `ffplay` for output.

```bash
cargo build --release
```

---

# How to use

## Start the sender

```bash
cargo run --release --bin ausha
```

It finds the default sink's monitor source and prints what receivers need:

```
capture: pulse source alsa_output.pci-0000_00_1f.3-....monitor
control: tcp/6996
media:   udp/6997 ssrc 1b60ad3b
pairing: xxxx-xxxx-xxxx
```

> **The pairing token is different every time the sender starts.** Copy the one
> your own sender just printed — a token from an earlier run, or from these
> examples, will be rejected. Pass `--token <token>` to pin it so you are not
> re-entering it each time:
>
> ```bash
> cargo run --release --bin ausha -- --token <token>
> ```

Find the address receivers should connect to with:

```bash
ip -4 addr show scope global | grep -oP 'inet \K[\d.]+'
```

---

## Play it on another computer

This is the full pipeline — jitter buffer, FEC, drift correction.

```bash
cargo run --release --bin ausha-recv -- --host <sender-ip> --token <token>
```

Substitute the address you found above and the `pairing:` line from your own
sender. The token may be typed with or without the dashes, in upper or lower
case — all four forms are accepted.

It prints a line every five seconds:

```
    5s  depth  80/ 80 ms  latency  81 ms  jitter   2.5 ms  loss  0.00%  \
        fec 0  plc 0  underruns 0  rate 1.0000
```

| Field | Meaning |
|---|---|
| `depth a/b` | Undecoded audio buffered, against the target it is aiming for |
| `latency` | Total buffered audio, the delay this receiver is adding |
| `jitter` | How irregularly packets are arriving |
| `loss` | Packets that never arrived |
| `fec` / `plc` | Lost frames rebuilt from redundancy, and ones papered over |
| `underruns` | Times the buffer ran dry. Should stay at zero |
| `rate` | Resampling ratio correcting clock drift. Sits near 1.0000 |

If `loss` is high but `underruns` stays at zero, it is working — the target
depth will grow on its own to absorb the bursts.

### Receiver options

```
--host <ip>             Sender address (required)
--token <token>         Pairing token the sender printed (required)
--control-port <port>   Sender control port (default 6996)
--name <name>           Name shown on the sender (default this host)
--sink <program>        pacat, aplay, ffplay, or null (default: first found)
--sink-latency <ms>     Requested device latency (default 20)
--run-for <seconds>     Exit after this long, for soak testing
--latency <preset>      low, balanced or stable (default balanced)
--simulate-loss <pct>   Drop this share of received packets, to exercise
                        concealment against a real sender
```

---

## Play it on Android

Build and install the app:

```bash
cd android
./gradlew installDebug
```

It needs the Android SDK and NDK; Gradle compiles the Rust core for each ABI
on the way, so the native library can never be stale. Set `ANDROID_NDK_HOME`
if yours is not at `~/Android/Sdk/ndk/28.2.13676358`, and pass
`-Pausha.abis=arm64-v8a` to build for phones only.

Pair the phone in whichever way suits:

- **Scan** — run `ausha --qr` and scan the code with the app's *Scan QR* button.
- **Tap a link** — the same `ausha://host:port?token=…` opens the app directly.
- **Pick from the list** — the app lists senders it finds over mDNS; tap one
  and type the token.
- **Type it** — address, port and token by hand.

The **Latency** control chooses how much the buffer holds: *Low* for the least
delay on a quiet network, *Balanced* for ordinary WiFi, *Stable* when the
signal is weak. The stats below show what playback is actually doing.

Playback continues with the screen off, holds a low-latency WiFi lock so the
radio does not park between beacons, and pauses for calls.

## Play it on Android without the app

mpv or VLC can receive a plain MPEG-TS stream with nothing to install and no
SDP file to copy across.

Find the phone's IP (Settings → About → Status), then:

```bash
cargo run --release --bin ausha -- --compat-ts <phone-ip>:1234
```

In mpv on the phone, open:

```
udp://0.0.0.0:1234
```

Repeat `--compat-ts` for more devices; they share a single encode.

Two things to know before using this on a network you do not control:

- **It skips the pairing token.** Anyone who can reach that address gets your
  desktop audio.
- **MPEG-TS has no loss recovery.** The Opus FEC that carries the real receiver
  through bad WiFi does nothing here, so expect glitches where `ausha-recv`
  would have none.

---

## Check it works, on one machine

```bash
cargo run --release --bin ausha -- --static-client 127.0.0.1:5555 --sdp-out /tmp/ausha.sdp
ffplay -protocol_whitelist file,rtp,udp -i /tmp/ausha.sdp
```

`--static-client` pushes to a fixed address with no handshake, and `--sdp-out`
writes the session description ffplay needs.

### Sender options

```
--control-port <port>   TCP control channel port (default 6996)
--media-port <port>     UDP media port receivers punch and listen on (default 6997)
--bitrate <kbps>        Opus bitrate (default 128)
--token <token>         Fixed pairing token instead of a freshly generated one
--static-client <addr>  Always send media to this ip:port, no handshake required
--sdp-out <path>        Write an SDP for --static-client, playable with ffplay
--compat-ts <ip:port>   Also push MPEG-TS to this address for mpv or VLC (repeatable)
--name <name>           Name advertised to receivers (default this host)
--no-discovery          Do not advertise over mDNS
--qr                    Print a QR code of the pairing link
```

---

## When it does not connect

Both of these look identical from the receiver — a handshake that times out —
and neither shows up when testing on one machine.

- **Firewall.** The sender needs inbound **TCP 6996** and **UDP 6997**:
  ```bash
  sudo ufw allow 6996/tcp && sudo ufw allow 6997/udp
  ```
- **Client isolation.** Many routers, and nearly all guest networks, block
  device-to-device traffic. If the sender answers `ping` but the handshake
  still times out, this is why. Use a different network.

Other symptoms:

| Symptom | Cause |
|---|---|
| `No PulseAudio monitor source found` | PulseAudio is not running |
| `rejected: invalid pairing token` | Token is from an earlier run. It changes every start — use the `pairing:` line the running sender printed, or pin it with `--token` |
| `no UDP punch received` | UDP 6997 is blocked while TCP 6996 got through |
| App lists no senders | mDNS is blocked on many routers. Scan the QR code or type the address |
| Audio glitches, `underruns` climbing | Genuinely bad link; try `--bitrate 96` |

---

## Tests

```bash
cargo test
```

The core suite runs the pipeline against real libopus with injected loss,
reordering and jitter bursts, and simulates hours of clock drift.
