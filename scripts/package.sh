#!/usr/bin/env bash
#
# Builds the Linux release artifacts into dist/.
#
# Native glibc binaries go into the .deb and .rpm, which declare their runtime
# dependencies and are the right thing for a distribution. A statically linked
# musl build goes into the tarball and the AppImage, which have no package
# manager to lean on and must run on a distro older than this one.

set -euo pipefail

readonly root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly build="$root/build/package"
readonly dist="$root/dist"
readonly musl_target=x86_64-unknown-linux-musl
readonly app_id=io.github.aaxis_em.ausha

readonly maintainer='Aaxis-em <aashishadhikari693@gmail.com>'
readonly homepage='https://aaxis-em.github.io/ausha/'
readonly summary='Stream desktop audio to receivers on the local network'

# Every timestamp written into an artifact comes from here, so that building the
# same commit twice produces the same bytes.
SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-$(git -C "$root" log -1 --format=%ct 2>/dev/null || date +%s)}"
export SOURCE_DATE_EPOCH

# The workspace version is the single source of truth for every artifact name.
version="$(sed -n '/^\[workspace\.package\]/,/^\[/{s/^version = "\(.*\)"/\1/p}' "$root/Cargo.toml")"
[[ -n $version ]] || { echo "cannot read version from Cargo.toml" >&2; exit 1; }
readonly version

# appimagetool is not packaged anywhere, so it is fetched once and pinned.
readonly appimagetool_url=https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-x86_64.AppImage
readonly appimagetool_sha256=ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0

say() { printf '\n\033[1m%s\033[0m\n' "$*" >&2; }
die() { printf '\033[31merror:\033[0m %s\n' "$*" >&2; exit 1; }

have() { command -v "$1" >/dev/null 2>&1; }

# Both builds publish the directory they produced, for stage() to read.
native_bin=''
musl_bin=''

build_native() {
    say "Building native binaries"
    cargo build --release --manifest-path "$root/Cargo.toml" -p player -p ausha-receiver
    native_bin="$root/target/release"
}

build_musl() {
    say "Building static musl binaries"
    have musl-gcc || die "musl-gcc not found. Install it: apt install musl-tools"
    rustup target list --installed | grep -qx "$musl_target" \
        || rustup target add "$musl_target"

    # opusic-sys compiles libopus with cmake, which needs pointing at musl-gcc
    # explicitly; it does not pick the target's linker up from cargo.
    CC_x86_64_unknown_linux_musl=musl-gcc \
    CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc \
        cargo build --release --target "$musl_target" \
        --manifest-path "$root/Cargo.toml" -p player -p ausha-receiver
    musl_bin="$root/target/$musl_target/release"
}

# The man pages carry the version in their .TH line, which is filled in here so
# that a release is one edit to Cargo.toml rather than one per page.
install_man() {
    local mandir="$1" page
    for page in ausha ausha-recv; do
        install -d "$mandir"
        sed "s/@VERSION@/$version/" "$root/packaging/$page.1" > "$mandir/$page.1"
        chmod 644 "$mandir/$page.1"
        gzip -9n "$mandir/$page.1"
    done
}

# Lays out the files every format installs, rooted at $1, with binaries from $2.
stage() {
    local dest="$1" bin="$2"

    install -Dm755 "$bin/ausha" "$dest/usr/bin/ausha"
    install -Dm755 "$bin/ausha-recv" "$dest/usr/bin/ausha-recv"

    install_man "$dest/usr/share/man/man1"

    install -Dm644 "$root/Readme.md" "$dest/usr/share/doc/ausha/Readme.md"
    install -Dm644 "$root/LICENSE" "$dest/usr/share/licenses/ausha/LICENSE"
}

# install(1) stamps files with the current time, which would leak into the
# archives and make two builds of one commit differ.
freeze_mtimes() {
    find "$1" -exec touch --no-dereference --date="@$SOURCE_DATE_EPOCH" {} +
}

tar_reproducibly() {
    local out="$1" dir="$2" name="$3"
    tar --create --sort=name --numeric-owner --owner=0 --group=0 \
        --mtime="@$SOURCE_DATE_EPOCH" -C "$dir" "$name" | gzip -9n > "$out"
}

build_deb() {
    local bin="$1"
    local dir="$build/deb/ausha_${version}_amd64"
    say "Building .deb"
    rm -rf "$dir"
    stage "$dir" "$bin"

    # Debian looks for the licence here, not under /usr/share/licenses.
    install -Dm644 "$root/packaging/copyright" "$dir/usr/share/doc/ausha/copyright"
    rm -rf "$dir/usr/share/licenses"

    install -d "$dir/DEBIAN"
    cat > "$dir/DEBIAN/control" <<EOF
Package: ausha
Version: $version
Architecture: amd64
Maintainer: $maintainer
Section: sound
Priority: optional
Homepage: $homepage
Depends: libc6 (>= 2.34), ffmpeg, pulseaudio-utils, iproute2
Suggests: alsa-utils
Description: $summary
 Ausha captures whatever this computer is already playing and streams it as
 Opus audio in RTP to receivers on the same network: the Ausha Android app, or
 ausha-recv on another computer.
 .
 Call mode runs the traffic the other way as well, publishing a receiver's
 microphone here as a capture device named ausha, so a call taken on this
 machine can be spoken into from a phone.
 .
 This package contains both the sender (ausha) and the desktop receiver
 (ausha-recv).
EOF
    freeze_mtimes "$dir"
    dpkg-deb --root-owner-group --build "$dir" "$dist/ausha_${version}_amd64.deb" >/dev/null
}

build_rpm() {
    local bin="$1"
    local top="$build/rpm"
    say "Building .rpm"
    have rpmbuild || die "rpmbuild not found. Install it: apt install rpm"

    rm -rf "$top"
    install -d "$top"/{SOURCES,BUILD,RPMS,SPECS}

    # The spec installs a prepared tree rather than compiling, so the sources it
    # unpacks are the staged files themselves.
    local payload="$build/rpm-payload/ausha-$version"
    rm -rf "$payload"
    stage "$payload" "$bin"
    freeze_mtimes "$payload"
    tar_reproducibly "$top/SOURCES/ausha-$version.tar.gz" "$(dirname "$payload")" "ausha-$version"

    rpmbuild --define "_topdir $top" --define "version $version" \
        -bb "$root/packaging/ausha.spec" >/dev/null
    find "$top/RPMS" -name '*.rpm' -exec cp {} "$dist/" \;
}

build_tarball() {
    local bin="$1"
    local name="ausha-$version-x86_64-linux-musl"
    local dir="$build/tar/$name"
    say "Building .tar.gz"
    rm -rf "$dir"

    install -Dm755 "$bin/ausha" "$dir/bin/ausha"
    install -Dm755 "$bin/ausha-recv" "$dir/bin/ausha-recv"
    install_man "$dir/share/man/man1"
    install -Dm644 "$root/LICENSE" "$dir/LICENSE"
    install -Dm644 "$root/Readme.md" "$dir/Readme.md"
    install -Dm755 "$root/packaging/install.sh" "$dir/install.sh"

    freeze_mtimes "$dir"
    tar_reproducibly "$dist/$name.tar.gz" "$(dirname "$dir")" "$name"
}

# Prints the path to appimagetool. `type -P` rather than `have`, so that this
# function does not match itself.
resolve_appimagetool() {
    local cached="$build/tools/appimagetool"
    local installed
    if installed="$(type -P appimagetool)"; then echo "$installed"; return; fi
    if [[ ! -x $cached ]]; then
        install -d "$(dirname "$cached")"
        curl -fsSL -o "$cached" "$appimagetool_url"
        echo "$appimagetool_sha256  $cached" | sha256sum -c - >/dev/null \
            || die "appimagetool checksum mismatch"
        chmod +x "$cached"
    fi
    echo "$cached"
}

build_appimage() {
    local bin="$1"
    local appdir="$build/AppDir"
    say "Building AppImage"
    rm -rf "$appdir"

    install -Dm755 "$bin/ausha" "$appdir/usr/bin/ausha"
    install -Dm755 "$bin/ausha-recv" "$appdir/usr/bin/ausha-recv"
    install -Dm755 "$root/packaging/appimage/AppRun" "$appdir/AppRun"
    install -Dm644 "$root/packaging/appimage/$app_id.appdata.xml" \
        "$appdir/usr/share/metainfo/$app_id.appdata.xml"

    # AppImage reads the desktop file and icon from the AppDir root; AppStream
    # and desktop integration want them in their FHS places. Both get a copy.
    install -Dm644 "$root/packaging/appimage/$app_id.desktop" "$appdir/$app_id.desktop"
    install -Dm644 "$root/packaging/appimage/$app_id.desktop" \
        "$appdir/usr/share/applications/$app_id.desktop"
    install -Dm644 "$root/assets/ausha-icon.png" "$appdir/$app_id.png"
    install -Dm644 "$root/assets/ausha-icon.png" \
        "$appdir/usr/share/icons/hicolor/512x512/apps/$app_id.png"
    install -Dm644 "$root/LICENSE" "$appdir/usr/share/licenses/ausha/LICENSE"

    ARCH=x86_64 "$(resolve_appimagetool)" --appimage-extract-and-run \
        "$appdir" "$dist/ausha-$version-x86_64.AppImage" >/dev/null
}

main() {
    local want=("$@")
    [[ ${#want[@]} -eq 0 ]] && want=(deb rpm tarball appimage)

    install -d "$dist"
    local format
    for format in "${want[@]}"; do
        case $format in
            deb|rpm)          [[ -n $native_bin ]] || build_native ;;
            tarball|appimage) [[ -n $musl_bin ]] || build_musl ;;
            *) die "unknown format: $format" ;;
        esac
    done

    for format in "${want[@]}"; do
        case $format in
            deb)      build_deb "$native_bin" ;;
            rpm)      build_rpm "$native_bin" ;;
            tarball)  build_tarball "$musl_bin" ;;
            appimage) build_appimage "$musl_bin" ;;
        esac
    done

    (cd "$dist" && find . -maxdepth 1 -type f ! -name SHA256SUMS -printf '%P\n' \
        | sort | xargs -r sha256sum > SHA256SUMS)

    say "Built into dist/"
    ls -lh "$dist"
}

main "$@"
