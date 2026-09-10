#!/usr/bin/env bash
# Runs several receivers against one sender and fails if any of them glitched.
#
# The core's own tests inject loss and jitter headlessly; this exercises what
# they cannot — one encoder, one fan-out and one registry serving several
# sessions at once over a real socket.

set -uo pipefail

clients=4
seconds=60
loss=0
latency=balanced
encrypt=()
token=soaksoaksoak

usage() {
    cat <<'USAGE'
soak.sh - run several receivers against one sender

Options:
  -n <clients>     Receivers to run at once (default 4)
  -d <seconds>     How long to play (default 60)
  -l <percent>     Drop this share of each receiver's packets (default 0)
  -p <preset>      Latency preset: low, balanced, stable (default balanced)
  -e               Encrypt the media path
  -h               Show this message
USAGE
}

while getopts "n:d:l:p:eh" option; do
    case "$option" in
        n) clients=$OPTARG ;;
        d) seconds=$OPTARG ;;
        l) loss=$OPTARG ;;
        p) latency=$OPTARG ;;
        e) encrypt=(--encrypt) ;;
        h) usage; exit 0 ;;
        *) usage; exit 2 ;;
    esac
done

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
out=$(mktemp -d)
trap 'kill "${sender_pid:-}" 2>/dev/null; wait "${sender_pid:-}" 2>/dev/null; rm -rf "$out"' EXIT

cargo build --release --manifest-path "$root/Cargo.toml" || exit 1

"$root/target/release/ausha" \
    --token "$token" --name soak --no-qr --no-discovery "${encrypt[@]}" \
    >"$out/sender.log" 2>&1 &
sender_pid=$!

# The sender has to be listening before the first hello, and ffmpeg needs a
# moment to open the capture device.
for _ in $(seq 30); do
    grep -q "^pairing:" "$out/sender.log" && break
    sleep 0.2
done
if ! kill -0 "$sender_pid" 2>/dev/null; then
    echo "sender exited during startup:" >&2
    cat "$out/sender.log" >&2
    exit 1
fi

echo "soak: $clients receivers, ${seconds}s, ${loss}% simulated loss, $latency${encrypt:+, encrypted}"

for id in $(seq 1 "$clients"); do
    "$root/target/release/ausha-recv" \
        --host 127.0.0.1 --token "$token" --name "soak-$id" \
        --sink null --latency "$latency" \
        --run-for "$seconds" --simulate-loss "$loss" \
        >"$out/recv-$id.log" 2>&1 &
done
wait $(jobs -p | grep -v "^$sender_pid$") 2>/dev/null

failures=0
printf '\n%-8s %10s %8s %8s %8s %10s %8s\n' client packets loss% fec plc underruns silence
for id in $(seq 1 "$clients"); do
    summary=$(grep "^played" "$out/recv-$id.log")
    if [[ -z $summary ]]; then
        printf '%-8s %s\n' "soak-$id" "no summary; see $out/recv-$id.log"
        failures=$((failures + 1))
        continue
    fi

    read -r packets lost recovered concealed underruns silence <<<"$(
        sed -E 's/.*: ([0-9]+) packets, ([0-9]+) lost \(([0-9]+) recovered by FEC, ([0-9]+) concealed\).*, ([0-9]+) underruns, ([0-9]+) silent.*/\1 \2 \3 \4 \5 \6/' <<<"$summary"
    )"
    printf '%-8s %10s %8.2f %8s %8s %10s %8s\n' \
        "soak-$id" "$packets" \
        "$(awk -v l="$lost" -v p="$packets" 'BEGIN { print p ? l * 100 / (p + l) : 0 }')" \
        "$recovered" "$concealed" "$underruns" "$silence"

    # Underruns and silent frames are the two things a listener actually
    # hears, so they are the pass condition rather than loss, which the
    # receiver is meant to absorb.
    if [[ $packets -eq 0 || $underruns -gt 0 || $silence -gt 0 ]]; then
        failures=$((failures + 1))
    fi
done

# Every receiver should have been served the same stream, not a share of it.
spread=$(grep -h "^played" "$out"/recv-*.log |
    sed -E 's/.*: ([0-9]+) packets.*/\1/' | sort -n | awk 'NR == 1 { min = $1 } END { print $1 - min }')
echo
echo "packet-count spread across receivers: $spread"

if [[ $failures -gt 0 ]]; then
    echo "FAILED: $failures of $clients receivers glitched (logs in $out)" >&2
    trap 'kill "${sender_pid:-}" 2>/dev/null' EXIT
    exit 1
fi
echo "OK: $clients receivers played ${seconds}s with no underruns and no silence"
