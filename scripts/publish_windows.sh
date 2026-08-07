#!/usr/bin/env bash
# =============================================================================
# Publish the Windows NSIS build to R2 — the Windows half of build_release.sh.
# Runs in CI (release-windows.yml, git-bash on a windows-latest runner) after
# `tauri build --bundles nsis`; also works locally on a Windows machine.
#
# Same distribution rules as macOS: FIXED artifact name (permanent download
# link), overwrite in place with no-cache, prune extras, and mutate ONLY the
# windows-x86_64 key of the shared latest.json (mac/linux entries preserved).
#
# Env: R2_ENDPOINT, R2_BUCKET (bucket/prefix), MUSIC_SYNC_BUCKET (public base),
# AWS_ACCESS_KEY_ID/AWS_SECRET_ACCESS_KEY (the house r2 token).
# =============================================================================
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# git-bash paths (/d/a/…) are opaque to the runner's native Windows Python and
# aws.exe — convert to mixed form (D:/a/…), which every tool understands.
if command -v cygpath >/dev/null 2>&1; then ROOT="$(cygpath -m "$ROOT")"; fi
BUNDLE_DIR="$ROOT/src-tauri/target/release/bundle/nsis"
TAURI_CONF="$ROOT/src-tauri/tauri.conf.json"
MERGE_MANIFEST="$ROOT/scripts/merge_manifest.py"

PY="$(command -v python3 || command -v python)"
AWS=(aws --endpoint-url "$R2_ENDPOINT")

die() { echo "ERROR: $*" >&2; exit 1; }

[[ -n "${R2_ENDPOINT:-}" && -n "${R2_BUCKET:-}" && -n "${MUSIC_SYNC_BUCKET:-}" ]] || die "R2 env missing"
[[ -f "$MERGE_MANIFEST" ]] || die "scripts/merge_manifest.py not found at $MERGE_MANIFEST"

VERSION="$("$PY" -c "import json;print(json.load(open('$TAURI_CONF'))['version'])")"

# Demand the installer for THIS version by name — never "newest wins". A cached
# target/ dir once let a stale 0.1.3 setup.exe be published under a 0.1.5
# manifest: the updater saw 0.1.5 > installed, downloaded, verified (the old
# signature matched the old bytes), reinstalled 0.1.3, and re-offered the update
# on every launch — a permanent update loop.
EXPECTED_SETUP="MusicSync_${VERSION}_x64-setup.exe"
SETUP_SRC="$BUNDLE_DIR/$EXPECTED_SETUP"
[[ -f "$SETUP_SRC" ]] || die "expected $EXPECTED_SETUP in $BUNDLE_DIR — found: $(ls "$BUNDLE_DIR" 2>/dev/null | tr '\n' ' ' || echo 'nothing'). Refusing to publish a stale bundle."
SIG_SRC="$SETUP_SRC.sig"
[[ -f "$SIG_SRC" ]] || die "missing updater signature: $SIG_SRC (createUpdaterArtifacts?)"

# The signature's trusted comment records the file it was made for. It is the
# only self-describing link between the bytes we upload and the version we
# advertise, so assert it rather than trust the filename.
SIG_FILE_NAME="$("$PY" - "$SIG_SRC" <<'PY'
import base64, re, sys
raw = base64.b64decode(open(sys.argv[1]).read().strip()).decode("utf-8", "replace")
m = re.search(r"file:(\S+)", raw)
print(m.group(1) if m else "")
PY
)"
[[ "$SIG_FILE_NAME" == "$EXPECTED_SETUP" ]] || die "signature is for '$SIG_FILE_NAME' but the manifest would say $VERSION (expected $EXPECTED_SETUP) — stale bundle, refusing to publish."

SETUP_NAME="MusicSync-setup.exe" # fixed — permanent download link
PLATFORM_KEY="windows-x86_64"
echo "==> v$VERSION · $PLATFORM_KEY · $(basename "$SETUP_SRC") → windows/$SETUP_NAME"

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
if command -v cygpath >/dev/null 2>&1; then STAGE="$(cygpath -m "$STAGE")"; fi
curl -fsS --max-time 15 "$MUSIC_SYNC_BUCKET/latest.json" -o "$STAGE/remote.json" 2>/dev/null || : > "$STAGE/remote.json"

# Shared merge implementation (also used by the mac script). It mutates only
# our platform key and REFUSES to lower the global manifest version (exit 2) —
# a stale-checkout CI run must fail the job, not un-publish the mac release.
"$PY" "$MERGE_MANIFEST" \
  --manifest "$STAGE/remote.json" \
  --out "$STAGE/latest.json" \
  --platform "$PLATFORM_KEY" \
  --version "$VERSION" \
  --signature "$(tr -d '\n' < "$SIG_SRC")" \
  --url "$MUSIC_SYNC_BUCKET/windows/$SETUP_NAME"

echo "==> Uploading"
"${AWS[@]}" s3 cp "$SETUP_SRC" "s3://$R2_BUCKET/windows/$SETUP_NAME" --cache-control "no-cache"
"${AWS[@]}" s3 cp "$STAGE/latest.json" "s3://$R2_BUCKET/latest.json" \
  --content-type application/json --cache-control "no-cache"

echo "==> Pruning windows/ (keeping $SETUP_NAME)"
("${AWS[@]}" s3 ls "s3://$R2_BUCKET/windows/" || true) | awk '{print $NF}' | while read -r obj; do
  [[ -z "$obj" || "$obj" == "$SETUP_NAME" ]] && continue
  echo "    rm windows/$obj"
  "${AWS[@]}" s3 rm "s3://$R2_BUCKET/windows/$obj"
done

# Read back what the world actually sees. `latest.json` is what every installed
# device polls, so a green build that published the wrong bytes must fail here,
# not on a user's machine.
echo "==> Verifying published manifest"
sleep 3
curl -fsS --max-time 20 "$MUSIC_SYNC_BUCKET/latest.json" -o "$STAGE/published.json" || die "cannot re-read latest.json"
PUBLISHED_FILE="$STAGE/published.json" EXPECTED_SETUP="$EXPECTED_SETUP" VERSION="$VERSION" \
PLATFORM_KEY="$PLATFORM_KEY" "$PY" <<'PY'
import base64, json, os, re, sys

data = json.load(open(os.environ["PUBLISHED_FILE"]))
want_version, want_file = os.environ["VERSION"], os.environ["EXPECTED_SETUP"]
if data.get("version") != want_version:
    sys.exit(f"ERROR: published manifest says {data.get('version')!r}, expected {want_version!r}")
sig = data["platforms"][os.environ["PLATFORM_KEY"]]["signature"]
raw = base64.b64decode(sig).decode("utf-8", "replace")
got = (re.search(r"file:(\S+)", raw) or [None, ""])[1]
if got != want_file:
    sys.exit(f"ERROR: published signature is for {got!r}, expected {want_file!r} — update loop shipped!")
print(f"OK: manifest {want_version} · signature for {got}")
PY

# The manifest can be right while the object is missing (failed upload, wrong
# prefix) — HEAD the public installer URL too.
curl -fsIL --max-time 20 "$MUSIC_SYNC_BUCKET/windows/$SETUP_NAME" >/dev/null \
  || die "installer not reachable at $MUSIC_SYNC_BUCKET/windows/$SETUP_NAME"

echo "==> Done — fixed download link: $MUSIC_SYNC_BUCKET/windows/$SETUP_NAME"
