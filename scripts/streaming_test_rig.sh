#!/usr/bin/env bash
# Local sync-proof rig for the RAOP multi-sender (Phase S2) — NO LP10 hardware.
#
# Proves the shared-NTP-anchor + matched-latency recipe keeps TWO independent RAOP
# receivers frame-locked, by streaming one identical test signal to both and
# cross-correlating their captured PCM to MEASURE the inter-receiver sync lag.
#
# Two modes (host receiver ports differ by platform — see below):
#
#   ./scripts/streaming_test_rig.sh            # HOST mode (design doc §5)
#       Two shairport-sync processes on this host (distinct ports), driven by the
#       REAL Rust engine via `cargo run --example stream_probe`. Works on a clean
#       host — Linux CI, or a Mac with the built-in AirPlay Receiver turned OFF.
#
#   ./scripts/streaming_test_rig.sh --docker   # DOCKER mode (host-independent)
#       Two shairport-sync receivers AND the cliraop sender run inside one Docker
#       bridge network (no host port/AirPlay conflicts, no NAT on the RAOP timing
#       path). The sender issues the identical cliraop command sequence the Rust
#       engine emits (shared `-ntp` anchor, matched `-w`/`-l`, PCM tee). Use this on
#       a dev Mac whose ControlCenter AirPlay Receiver permanently holds :5000.
#
# Requires: python3 + numpy (measurement). HOST mode also needs shairport-sync
# (brew/apt) and the cliraop binary (scripts/fetch_cliraop.sh). DOCKER mode needs
# only Docker. The measured lag prints at the end; target is tens of ms or better.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
TAURI_DIR="$PROJECT_ROOT/src-tauri"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/musicsync-rig.XXXXXX")"
LIBRAOP_SHA="52e705106d3b4149c7f37ee643b69f96944e5786"
SECS="${SECS:-12}"
FREQ="${FREQ:-1000}"
MODE="host"
[[ "${1:-}" == "--docker" ]] && MODE="docker"

echo "== MusicSync S2 streaming test rig (mode: $MODE) =="
echo "work dir: $WORK"

if ! python3 -c "import numpy" >/dev/null 2>&1; then
  echo "ERROR: python3 + numpy required for the sync measurement." >&2
  exit 1
fi

# ─────────────────────────────────────────────────────────────────────────────
# The cross-correlation analyzer, shared by both modes. Robust to (a) the pure
# sine's periodicity and (b) each receiver's independent output-buffer lead-in:
# it measures each capture's leading silence, aligns both to their first audio,
# then reports the residual offset AND the drift across the whole clip.
# ─────────────────────────────────────────────────────────────────────────────
measure() {
  local capA="$1" capB="$2"
  CAP_A="$capA" CAP_B="$capB" python3 - <<'PY'
import os, sys, numpy as np
RATE=44100
def mono(p):
    x=np.fromfile(p,dtype="<i2"); n=(len(x)//2)*2
    if n < RATE*2:
        print(f"FAIL: too little captured audio in {p} ({n} samples). "
              "Check receiver logs — cliraop may not have connected/decoded.")
        sys.exit(1)
    return x[:n].astype(np.float64).reshape(-1,2).sum(axis=1)
a=mono(os.environ["CAP_A"]); b=mono(os.environ["CAP_B"])
def env(x,win=128): return np.sqrt(np.convolve(x*x,np.ones(win)/win,mode="same"))
ea,eb=env(a),env(b)
print(f"capture A: {len(a)/RATE:6.2f}s   capture B: {len(b)/RATE:6.2f}s")
def lead(e):
    thr=0.2*e.max(); i=np.where(e>thr)[0]; return int(i[0]) if len(i) else -1
la,lb=lead(ea),lead(eb)
print(f"onset A: {la} smp ({la/RATE*1000:8.2f} ms)   onset B: {lb} smp ({lb/RATE*1000:8.2f} ms)")
print(f"raw onset offset (incl. per-receiver output lead-in): {(la-lb)/RATE*1000:+.3f} ms")
# Align both to their own first audio, then envelope cross-correlate the aligned
# signals: residual = true relative playback offset once the lead-in is removed.
na=ea[la:]; nb=eb[lb:]; m=min(len(na),len(nb)); na=na[:m]; nb=nb[:m]
na=(na-na.mean())/(na.std()+1e-9); nb=(nb-nb.mean())/(nb.std()+1e-9)
ml=int(0.05*RATE)  # ±50 ms fine search
corr=np.correlate(na,nb[ml:-ml],mode="valid"); resid=np.argmax(corr)-ml
# Drift: compare envelope xcorr on the first vs last second of the aligned clip.
def seg_lag(x,y):
    x=(x-x.mean())/(x.std()+1e-9); y=(y-y.mean())/(y.std()+1e-9)
    c=np.correlate(x,y[ml:-ml],mode="valid"); return np.argmax(c)-ml
first=seg_lag(na[:RATE], nb[:RATE]) if m>2*RATE else resid
last =seg_lag(na[-RATE:],nb[-RATE:]) if m>2*RATE else resid
drift=last-first
print(f"post-lead-in residual offset: {resid:+d} smp ({resid/RATE*1000:+.3f} ms)")
print(f"drift over clip: {drift:+d} smp ({drift/RATE*1000:+.3f} ms)")
print()
print(f"==> INTER-RECEIVER SYNC LAG (locked, lead-in removed): {abs(resid/RATE*1000):.3f} ms")
print(f"==> DRIFT: {abs(drift/RATE*1000):.3f} ms   (target: ~0)")
PY
}

# ─────────────────────────────────────────────────────────────────────────────
if [[ "$MODE" == "host" ]]; then
  if ! command -v shairport-sync >/dev/null 2>&1; then
    echo "shairport-sync not found; installing…"
    if command -v brew >/dev/null 2>&1; then brew install shairport-sync
    elif command -v apt-get >/dev/null 2>&1; then sudo apt-get install -y shairport-sync
    else echo "ERROR: install shairport-sync manually." >&2; exit 1; fi
  fi
  echo "shairport-sync: $(shairport-sync -V 2>&1 | head -1)"
  if ! ls "$TAURI_DIR"/binaries/cliraop-* >/dev/null 2>&1; then
    echo "cliraop missing; fetching…"; "$SCRIPT_DIR/fetch_cliraop.sh"
  fi

  PORT_A="${PORT_A:-5000}"; PORT_B="${PORT_B:-5010}"
  CAP_A="$WORK/recv_A.pcm"; CAP_B="$WORK/recv_B.pcm"
  PIDS=()
  cleanup_host(){ for p in "${PIDS[@]:-}"; do [[ -n "$p" ]] && kill "$p" 2>/dev/null||true; done; wait 2>/dev/null||true; }
  trap cleanup_host EXIT

  echo "── launching 2 shairport-sync receivers (ports $PORT_A/$PORT_B) ──"
  # NOTE: shairport-sync's classic-AirPlay RTSP port is 5000 on some builds and
  # is only changeable where the build honors general.port. On a Mac whose
  # ControlCenter AirPlay Receiver holds :5000, these binds fail — use --docker.
  shairport-sync -a "FakeLP10-A" -o stdout -p "$PORT_A" >"$CAP_A" 2>"$WORK/logA" & PIDS+=("$!")
  shairport-sync -a "FakeLP10-B" -o stdout -p "$PORT_B" >"$CAP_B" 2>"$WORK/logB" & PIDS+=("$!")
  sleep 3
  for L in "$WORK/logA" "$WORK/logB"; do
    if grep -qi "could not establish a service" "$L" 2>/dev/null; then
      echo
      echo "!! A receiver could not bind its RTSP port (another AirPlay receiver is"
      echo "   holding it — on macOS disable System Settings ▸ General ▸ AirDrop &"
      echo "   Handoff ▸ AirPlay Receiver, or run this rig with --docker)."
      exit 2
    fi
  done
  command -v dns-sd >/dev/null 2>&1 && { echo "── _raop._tcp advertisements ──"; (dns-sd -B _raop._tcp & D=$!; sleep 2; kill $D 2>/dev/null) 2>/dev/null | grep -i raop || true; }

  echo "── streaming ${SECS}s ${FREQ}Hz tone to both via the Rust engine ──"
  ( cd "$TAURI_DIR" && STREAM_PROBE_SECS="$SECS" STREAM_PROBE_FREQ="$FREQ" \
      cargo run --quiet --example stream_probe -- "127.0.0.1:$PORT_A" "127.0.0.1:$PORT_B" )
  sleep 1
  echo "── measuring ──"; measure "$CAP_A" "$CAP_B"
  echo "receiver logs: $WORK/logA , $WORK/logB"
  exit 0
fi

# ─────────────────────────────────── DOCKER ─────────────────────────────────
command -v docker >/dev/null 2>&1 || { echo "ERROR: docker required for --docker mode." >&2; exit 1; }
docker info >/dev/null 2>&1 || { echo "ERROR: Docker daemon not running." >&2; exit 1; }

IMG="musicsync-shairport-rig"
BUILD="$WORK/img"; mkdir -p "$BUILD"
cat > "$BUILD/entrypoint.sh" <<'EOF'
#!/bin/sh
set -e
mkdir -p /var/run/dbus
dbus-daemon --system --fork 2>/dev/null || true
avahi-daemon --no-chroot -D 2>/dev/null || true
sleep 1
NAME="${SP_NAME:-FakeLP10}"
rm -f /tmp/sp.fifo; mkfifo /tmp/sp.fifo
( stdbuf -o0 cat /tmp/sp.fifo > /root/audio.pcm ) &
cat > /sp.conf <<CONF
general = { name = "$NAME"; port = 5000; };
diagnostics = { log_verbosity = 0; };
pipe = { name = "/tmp/sp.fifo"; };
CONF
exec shairport-sync -c /sp.conf -o pipe
EOF
cat > "$BUILD/Dockerfile" <<'EOF'
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
      shairport-sync avahi-daemon dbus libatomic1 && rm -rf /var/lib/apt/lists/*
COPY entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh
ENTRYPOINT ["/entrypoint.sh"]
EOF
echo "── building receiver image ($IMG) ──"
docker build -q -t "$IMG" "$BUILD" >/dev/null

# Linux cliraop (matches the pinned libraop the app bundles); arch-appropriate.
UNAME_M="$(uname -m)"
case "$UNAME_M" in
  arm64|aarch64) RAOP_ASSET="cliraop-linux-aarch64" ;;
  x86_64|amd64)  RAOP_ASSET="cliraop-linux-x86_64" ;;
  *)             RAOP_ASSET="cliraop-linux-x86_64" ;;
esac
CLIRAOP="$WORK/$RAOP_ASSET"
echo "── fetching $RAOP_ASSET (libraop@${LIBRAOP_SHA:0:10}) ──"
curl -fsSL "https://github.com/philippe44/libraop/raw/$LIBRAOP_SHA/bin/$RAOP_ASSET" -o "$CLIRAOP"
chmod +x "$CLIRAOP"

# Test signal: click train (sharp, non-periodic onsets → unambiguous alignment).
TONE="$WORK/signal.pcm"
python3 - "$TONE" "$SECS" "$FREQ" <<'PY'
import sys, numpy as np
RATE=44100; path,secs,freq=sys.argv[1],float(sys.argv[2]),float(sys.argv[3])
n=int(RATE*(secs+3)); t=np.arange(n)/RATE
env=((t%0.4)<0.03).astype(float)
sig=(0.5*np.sin(2*np.pi*freq*t))*env
np.repeat((sig*16000).astype("<i2")[:,None],2,axis=1).reshape(-1).tofile(path)
PY

# Sender script: the EXACT command sequence StreamEngine emits (shared -ntp
# anchor captured once, matched -w/-l, identical PCM teed to every child).
SEND="$WORK/send.sh"
cat > "$SEND" <<EOF
#!/bin/sh
set -e
C=/cliraop
\$C -ntp /tmp/anchor.ntp
echo "shared NTP anchor = \$(cat /tmp/anchor.ntp)"
\$C -nf /tmp/anchor.ntp -w 1500 -l 22050 -p 5000 spA - < /signal.pcm 2>/tmp/a.log &
PA=\$!
\$C -nf /tmp/anchor.ntp -w 1500 -l 22050 -p 5000 spB - < /signal.pcm 2>/tmp/b.log &
PB=\$!
sleep $((SECS + 4))
kill \$PA \$PB 2>/dev/null || true
echo "sender done"
EOF
chmod +x "$SEND"

NET="musicsync-raopnet-$$"
cleanup_docker(){ docker rm -f spA spB raopsender >/dev/null 2>&1||true; docker network rm "$NET" >/dev/null 2>&1||true; }
trap cleanup_docker EXIT
docker network create "$NET" >/dev/null 2>&1 || true

echo "── starting 2 receivers on an isolated bridge network ──"
docker run -d --network "$NET" --name spA -e SP_NAME=FakeLP10-A "$IMG" >/dev/null
docker run -d --network "$NET" --name spB -e SP_NAME=FakeLP10-B "$IMG" >/dev/null
sleep 4

echo "── streaming to both (engine recipe: shared anchor, matched latency, PCM tee) ──"
docker run --rm --network "$NET" --name raopsender --entrypoint /send.sh \
  -v "$CLIRAOP:/cliraop:ro" -v "$TONE:/signal.pcm:ro" -v "$SEND:/send.sh:ro" \
  "$IMG"
sleep 2

echo "── retrieving captures ──"
docker stop -t 4 spA spB >/dev/null 2>&1 || true
docker cp spA:/root/audio.pcm "$WORK/capA.pcm"
docker cp spB:/root/audio.pcm "$WORK/capB.pcm"
echo "── measuring ──"
measure "$WORK/capA.pcm" "$WORK/capB.pcm"
