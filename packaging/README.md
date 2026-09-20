# Packaging the Linux binaries

Two programs are shipped together: `ausha`, the sender, and `ausha-recv`, the
desktop receiver. Everything here builds both into one package.

```bash
./scripts/package.sh              # all four formats into dist/
./scripts/package.sh deb tarball  # or just the ones named
```

| Artifact | Built for | Linkage |
|---|---|---|
| `ausha_<ver>_amd64.deb` | Debian, Ubuntu, Mint | glibc, deps declared |
| `ausha-<ver>-1.x86_64.rpm` | Fedora, RHEL, openSUSE | glibc, deps declared |
| `ausha-<ver>-x86_64-linux-musl.tar.gz` | any distro | static, no deps |
| `ausha-<ver>-x86_64.AppImage` | any distro | static, no deps |

| File | What it is |
|---|---|
| `ausha.1`, `ausha-recv.1` | Man pages. The only user documentation the packages install. |
| `copyright` | Debian's machine-readable copyright file, required by policy. |
| `ausha.spec` | RPM spec. Installs the tree `package.sh` stages; it does not compile. |
| `install.sh` | Shipped inside the tarball, for people with no package manager route. |
| `aur/PKGBUILD` | Arch. Builds from the release tag, unlike the others. |
| `appimage/AppRun` | AppImage entry point. Dispatches to one binary or the other. |

## Why glibc for the packages and musl for the rest

A `.deb` or `.rpm` has a dependency solver behind it, so dynamic linking is
correct: the package says `libc6 (>= 2.34)` and the distribution guarantees it.
The tarball and the AppImage have nothing behind them and get run on whatever
the user has, so they are linked statically against musl and depend on no
shared library at all.

This matters more than it sounds. A glibc binary built on Ubuntu 24.04 needs
glibc 2.39 and will not start on Debian 12, Ubuntu 22.04 or RHEL 9 — it fails
with a version error rather than anything a user can act on.

## Build prerequisites

```bash
sudo apt install cmake musl-tools rpm    # Debian/Ubuntu
```

`musl-tools` provides `musl-gcc`, which the static build needs because
`opusic-sys` compiles libopus from C with cmake and cargo does not supply a C
cross-compiler. `rpm` provides `rpmbuild`. `appimagetool` is fetched on first
use into `build/package/tools/`, pinned by version and checksum.

Without those, `./scripts/package.sh deb` still works on its own.

## Runtime dependencies, and why they are not bundled

| Needed by | Program | Debian | Fedora | Arch |
|---|---|---|---|---|
| sender: capture and Opus encode | `ffmpeg` | `ffmpeg` | `ffmpeg`/`ffmpeg-free` | `ffmpeg` |
| sender: find the monitor source, publish the uplink device | `pactl` | `pulseaudio-utils` | `pulseaudio-utils` | `libpulse` |
| sender: mDNS advertisement | `ip` | `iproute2` | `iproute` | `iproute2` |
| receiver: output | `pacat`, `aplay` or `ffplay` | as above, or `alsa-utils` | as above | as above |

These stay external on purpose. The sender shells out to `ffmpeg` and drives
the user's own PulseAudio or PipeWire daemon; bundling either would mean
bundling a media stack and then failing to talk to the session's real audio
server.

The static tarball and AppImage are therefore self-contained as *binaries*, not
as installations: they still need `ffmpeg` and `pactl` on the host. `install.sh`
says so when they are missing.

## Cutting a release

The version lives in `[workspace.package]` in the root `Cargo.toml`, and
`package.sh` reads it from there — every artifact name and the man pages'
`.TH` line follow. Four other files carry it independently and have to be
bumped by hand:

| File | What to change |
|---|---|
| `Cargo.toml` | `[workspace.package] version` — everything Linux follows this |
| `packaging/aur/PKGBUILD` | `pkgver`, and reset `pkgrel=1` |
| `packaging/ausha.spec` | a new `%changelog` entry |
| `packaging/appimage/*.appdata.xml` | a new `<release>` entry |
| `android/app/build.gradle.kts` | `versionCode` and `versionName`, if the app changed |

The Android app versions independently of the binaries, so bump it only when it
actually changed; `fdroid/README.md` covers the rest of that side.

Then tag and let CI build it:

```bash
git tag -s v0.1.0 && git push --tags
```

`.github/workflows/release.yml` refuses a tag that does not match the workspace
version, builds all four artifacts and attaches them to the GitHub release.
Building locally with `./scripts/package.sh` produces the same bytes: artifact
timestamps come from the commit date, so two builds of one commit are identical.

## Publishing

`dist/` is what goes on a GitHub release. Attach all four artifacts plus
`SHA256SUMS`.

- **Debian/Ubuntu and Fedora**: the `.deb` and `.rpm` are direct downloads. A
  proper apt or dnf repository is a separate job and is not set up.
- **Arch**: `aur/PKGBUILD` goes in the AUR git repository for `ausha`, not here.
  Run `updpkgsums` to replace the `SKIP` checksum once the tag exists, then
  `makepkg --printsrcinfo > .SRCINFO`, which the AUR requires alongside it.
- **Android**: see [../fdroid/README.md](../fdroid/README.md). F-Droid builds
  from the tag itself and takes no artifact from `dist/`.

## Flatpak

Not provided, and not a good fit for the sender. Call mode runs
`pactl load-module module-pipe-source` against the host daemon and spawns
`ffmpeg`; the Flatpak sandbox permits neither, so the uplink cannot work. The
receiver alone would package cleanly, but shipping half the tool under a
different mechanism is worse than shipping none of it.
