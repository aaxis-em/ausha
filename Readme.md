<p align="center">
  <img src="assets/ausha@2x.png" alt="" width="89">
</p>

<h1 align="center">Ausha</h1>

<p align="center">
  Play your computer's sound on your phone — and use your phone as the
  computer's microphone.
</p>

<p align="center">
  <a href="https://aaxis-em.github.io/ausha/">Website</a>
</p>

---

## Two things it does

**Listen.** Whatever your computer is playing — music, a video, a call — comes
out of your phone, or another computer, on the same WiFi.

**Call mode.** Your phone's microphone goes back the other way, so the phone
becomes a wireless headset for a call you take on the computer.

## How it works

```
       your computer                              your phone
   ┌──────────────────┐                      ┌──────────────────┐
   │  music, video,   │ ─────  sound  ─────▶ │     speaker      │
   │    your call     │                      │                  │
   │                  │ ◀──  your voice  ─── │    microphone    │
   └──────────────────┘      (call mode)     └──────────────────┘
                            same WiFi
```

The computer captures what it is already playing and sends it to the phone. In
call mode the phone sends its microphone back, and the computer offers that to
your call app as an input device named **ausha**.

---

# Install

> **Nothing is published yet.** There is no release, no AUR package and no
> F-Droid listing so far, so for the moment build from source — the commands
> below are what the first release will look like. See
> [packaging/README.md](packaging/README.md) and
> [fdroid/README.md](fdroid/README.md) for where that stands.

## On the computer

Debian, Ubuntu or Mint:

```bash
sudo apt install ./ausha_0.1.0_amd64.deb
```

Fedora, RHEL or openSUSE:

```bash
sudo dnf install ./ausha-0.1.0-1.x86_64.rpm
```

Arch, from the [AUR](https://aur.archlinux.org/packages/ausha):

```bash
paru -S ausha
```

Any other distribution — a static build that needs no libraries:

```bash
tar xf ausha-0.1.0-x86_64-linux-musl.tar.gz
cd ausha-0.1.0-x86_64-linux-musl
sudo ./install.sh                   # or: prefix=$HOME/.local ./install.sh
```

There is an AppImage too, if you would rather not install anything:

```bash
chmod +x ausha-0.1.0-x86_64.AppImage
./ausha-0.1.0-x86_64.AppImage              # the sender
./ausha-0.1.0-x86_64.AppImage recv --help  # the receiver
```

Downloads are on the [releases page](https://github.com/aaxis-em/ausha/releases).
Either way you need **ffmpeg** built with libopus and **pactl** on the machine;
the packages pull both in, the tarball and AppImage tell you if they are
missing.

Build from source, which needs Rust and cmake, and is the only route today:

```bash
cargo build --release
```

That leaves the binaries in `target/release/`. To build the packages themselves,
see [packaging/README.md](packaging/README.md).

## On the phone

The Android app is in [`android/`](android/). Build it as [step 2
below](#2-put-the-app-on-your-phone) describes.

---

# How to use it

## 1. Start it on the computer

To listen only:

```bash
ausha
```

For calls as well:

```bash
ausha --uplink
```

It prints a pairing code and a QR code. Leave it running.

> Needs Linux and ffmpeg built with `libopus`. The Windows sender is written
> but has never been run.

## 2. Put the app on your phone

Build it yourself:

1. Open the `android/` folder in Android Studio.
2. On the phone, turn on **USB debugging**, and plug it into the computer.
3. Press **Run**.

Or from the command line, which needs the Android SDK, the NDK and
[`cargo-ndk`](https://github.com/bbqsrc/cargo-ndk):

```bash
cd android && ./gradlew :app:installDebug
```

> The app is packaged for **F-Droid** but not yet submitted; see
> [fdroid/README.md](fdroid/README.md) for where that stands.

## 3. Connect

Tap **Scan QR** in the app and point it at the code on your screen. Sound
starts straight away.

**For calls**, also turn on **Call mode** in the app, then pick **ausha** as
the microphone in Zoom, Discord, Meet — whatever you are calling from.

> **Use headphones on the phone for calls.** Speakerphone echo cancellation
> has not been confirmed on real hardware yet, so without headphones the
> person you are talking to may hear themselves.

## Or listen on another computer

No app needed. Use the address and code the sender printed:

```bash
ausha-recv --host <ip> --token <code>
```

---

## Options

On the computer, `ausha`:

| Flag | What it does |
|---|---|
| `--uplink` | Also take the phone's microphone. Needed for call mode |
| `--encrypt` | Encrypt the audio |
| `--token <code>` | Reuse a pairing code instead of a new one each start |
| `--bitrate <kbps>` | Audio quality. Default 128 |
| `--no-qr` | Do not print the QR code |
| `--help` | Everything else |

On another computer, `ausha-recv`:

| Flag | What it does |
|---|---|
| `--host <ip>` and `--token <code>` | Required. Both are on the sender's screen |
| `--latency low`, `balanced` or `stable` | Less delay, or more resilience on bad WiFi |
| `--help` | Everything else |

---

## If it does not connect

| What you see | What it is |
|---|---|
| It just times out | The firewall. Allow TCP 6996 and UDP 6997 |
| It times out, but `ping` works | Your WiFi blocks device-to-device traffic. Common on guest networks — use another one |
| `invalid pairing token` | The code changes every time the sender starts. Use the one on screen now, or pin it with `--token` |
| No senders listed in the app | Your router blocks mDNS. Scan the QR code instead |
| Crackling, or `underruns` counting up | The link is genuinely struggling. Try `--bitrate 96`, or `--latency stable` on the receiver |
| Call mode says no microphone | The sender was started without `--uplink` |

---

## More

- **[arch.md](arch.md)** — how it is built, and how to run it the awkward ways:
  playing the stream in `ffplay` or mpv, encrypting it, soaking several
  receivers at once.
- **[plan.md](plan.md)** — what is done and what is next.
- **[packaging/README.md](packaging/README.md)** — building the Linux packages,
  and what each format is for.
- **[fdroid/README.md](fdroid/README.md)** — cutting a release of the Android
  app and publishing it on F-Droid.
- Tests: `cargo test`.

---

## License

[GPL-3.0-or-later](LICENSE).
