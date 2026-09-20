#!/bin/sh
# Installs the contents of this directory. Set prefix= to install elsewhere,
# and DESTDIR= to stage into a directory without touching the live system.
#
#   ./install.sh                      -> /usr/local, needs root
#   prefix=$HOME/.local ./install.sh  -> no root needed

set -eu

prefix="${prefix:-/usr/local}"
destdir="${DESTDIR:-}"
here="$(cd "$(dirname "$0")" && pwd)"

target="$destdir$prefix"

if ! mkdir -p "$target/bin" 2>/dev/null; then
    echo "cannot write to $target — run with sudo, or set prefix=\$HOME/.local" >&2
    exit 1
fi

install -m755 "$here/bin/ausha" "$here/bin/ausha-recv" "$target/bin/"

mkdir -p "$target/share/man/man1"
install -m644 "$here"/share/man/man1/*.1.gz "$target/share/man/man1/"

mkdir -p "$target/share/licenses/ausha" "$target/share/doc/ausha"
install -m644 "$here/LICENSE" "$target/share/licenses/ausha/"
install -m644 "$here/Readme.md" "$target/share/doc/ausha/"

echo "Installed ausha and ausha-recv into $target/bin"

case ":$PATH:" in
    *":$prefix/bin:"*) ;;
    *) echo "note: $prefix/bin is not on your PATH" ;;
esac

for tool in ffmpeg pactl; do
    command -v "$tool" >/dev/null 2>&1 \
        || echo "note: $tool is not installed; the sender needs it"
done
