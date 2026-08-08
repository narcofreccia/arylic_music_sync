#!/usr/bin/env bash
# Fetch the prebuilt `cliraop` RAOP-sender binaries from philippe44/libraop and
# install them into src-tauri/binaries/ under the Tauri externalBin naming
# convention (cliraop-<target-triple>). This is the "fetch, not build" analogue
# of tide_share's scripts/build_sidecar.sh — libraop ships working prebuilt CLI
# binaries for every target we need, so CI needs NO C toolchain.
#
# Usage:
#   ./scripts/fetch_cliraop.sh            # fetch for the current host triple only
#   ./scripts/fetch_cliraop.sh --all      # fetch every target triple (CI packaging)
#   ./scripts/fetch_cliraop.sh --list     # print the host→libraop asset mapping
#
# The libraop commit is PINNED below for reproducibility. The binaries are large
# (5-6 MB each) and are gitignored under src-tauri/binaries/ — this script (run
# in CI, or locally for the test rig) is the source of truth for which files land.

set -euo pipefail

# --- pinned libraop revision -------------------------------------------------
# Binaries live in the repo's bin/ directory (not GitHub Releases). Pin a commit
# SHA so a re-run always fetches byte-identical senders.
LIBRAOP_REPO="philippe44/libraop"
LIBRAOP_SHA="52e705106d3b4149c7f37ee643b69f96944e5786"   # master @ 2026-08-04

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
BIN_DIR="$PROJECT_ROOT/src-tauri/binaries"
RAW_BASE="https://github.com/${LIBRAOP_REPO}/raw/${LIBRAOP_SHA}/bin"

# --- map Rust target triple → libraop asset filename -------------------------
# Left  = Tauri target-triple (the name we install as, sans .exe on unix).
# Right = filename in libraop's bin/. Extend as new platforms are supported.
declare -a TRIPLES=(
  "aarch64-apple-darwin=cliraop-macos-arm64"
  "x86_64-apple-darwin=cliraop-macos-x86_64"
  "x86_64-unknown-linux-gnu=cliraop-linux-x86_64"
  "aarch64-unknown-linux-gnu=cliraop-linux-aarch64"
  "i686-pc-windows-msvc=cliraop.exe"
  "x86_64-pc-windows-msvc=cliraop.exe"
)

host_triple() {
  if command -v rustc >/dev/null 2>&1; then
    rustc -vV | awk '/host:/ {print $2}'
  else
    echo ""
  fi
}

asset_for() {
  local triple="$1"
  for entry in "${TRIPLES[@]}"; do
    if [[ "${entry%%=*}" == "$triple" ]]; then
      echo "${entry#*=}"
      return 0
    fi
  done
  return 1
}

fetch_one() {
  local triple="$1"
  local asset
  if ! asset="$(asset_for "$triple")"; then
    echo "WARN: no libraop asset mapped for triple '$triple' — skipping" >&2
    return 0
  fi
  local dest="$BIN_DIR/cliraop-${triple}"
  [[ "$asset" == *.exe ]] && dest="${dest}.exe"

  echo "fetching $asset  ->  $(basename "$dest")"
  mkdir -p "$BIN_DIR"
  curl -fsSL "${RAW_BASE}/${asset}" -o "$dest"
  chmod +x "$dest" || true

  # Windows senders ship two OpenSSL 1.1 DLLs alongside cliraop.exe; bundle them
  # next to the binary so the sender loads on a clean Windows host.
  if [[ "$asset" == *.exe ]]; then
    for dll in libcrypto-1_1.dll libssl-1_1.dll; do
      echo "  + $dll"
      curl -fsSL "${RAW_BASE}/${dll}" -o "$BIN_DIR/$dll" || \
        echo "  WARN: could not fetch $dll" >&2
    done
  fi
}

case "${1:-}" in
  --list)
    echo "libraop pinned at ${LIBRAOP_REPO}@${LIBRAOP_SHA}"
    printf '%s\n' "${TRIPLES[@]}"
    exit 0
    ;;
  --all)
    for entry in "${TRIPLES[@]}"; do
      fetch_one "${entry%%=*}"
    done
    ;;
  "")
    HOST="$(host_triple)"
    if [[ -z "$HOST" ]]; then
      echo "ERROR: rustc not found; pass an explicit triple or use --all" >&2
      exit 1
    fi
    fetch_one "$HOST"
    ;;
  *)
    fetch_one "$1"
    ;;
esac

echo
echo "Installed cliraop binaries in $BIN_DIR:"
ls -la "$BIN_DIR" 2>/dev/null || true
