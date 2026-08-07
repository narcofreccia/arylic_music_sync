#!/usr/bin/env bash
# =============================================================================
# MusicSync macOS release — build, sign, notarize, publish to R2.
#
# House pipeline (port of tide_pos2/scripts/build_release.sh for a flat
# single-package Tauri 2 repo: src/ + src-tauri/ at the repo root, no
# workspace, no sidecars).
#
#   ./scripts/build_release.sh                 full release
#   ./scripts/build_release.sh --dry-run       preflight + plan, change nothing
#   ./scripts/build_release.sh --skip-notarize unsigned local build (dev only)
#   ./scripts/build_release.sh --skip-upload   build + manifest, no R2 writes
#   ./scripts/build_release.sh --manifest-only re-publish manifest from the
#                                              newest already-built artifacts
#   ./scripts/build_release.sh --force         republish an already-live version
#
# Bucket layout (music_sync/ prefix in the shared house `storage` R2 bucket,
# same bucket + `r2` token as tide_reel/tide_share; see docs/RELEASING.md):
#   s3://storage/music_sync/latest.json  one manifest, ALL platform keys — this
#                                        script mutates only darwin-<arch>,
#                                        preserving the windows/linux entries
#   s3://storage/music_sync/mac/MusicSync.dmg               (installer — FIXED link)
#   s3://storage/music_sync/mac/MusicSync_<arch>.app.tar.gz (updater — fixed name)
# Only the LATEST release is hosted: names are stable so download links never
# change, each release overwrites in place (uploaded no-cache so the r2.dev
# edge revalidates) and anything else under mac/ is pruned.
#
# One-time setup: docs/RELEASING.md (updater keypair, R2 token scope, .env.build).
# =============================================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
TAURI_CONF="$ROOT/src-tauri/tauri.conf.json"
BUNDLE_DIR="$ROOT/src-tauri/target/release/bundle"
APP_BUNDLE="$BUNDLE_DIR/macos/MusicSync.app"
MERGE_MANIFEST="$SCRIPT_DIR/merge_manifest.py"

# ---- flags ------------------------------------------------------------------
DRY_RUN=0 SKIP_NOTARIZE=0 SKIP_UPLOAD=0 MANIFEST_ONLY=0 FORCE=0
for arg in "$@"; do
  case "$arg" in
    --dry-run) DRY_RUN=1 ;;
    --skip-notarize) SKIP_NOTARIZE=1 ;;
    --skip-upload) SKIP_UPLOAD=1 ;;
    --manifest-only) MANIFEST_ONLY=1 ;;
    --force) FORCE=1 ;;
    *) echo "unknown flag: $arg"; exit 1 ;;
  esac
done

log() { printf '\033[1;36m==>\033[0m %s\n' "$*"; }
die() { printf '\033[1;31mERROR:\033[0m %s\n' "$*" >&2; exit 1; }

# ---- env --------------------------------------------------------------------
# .env.build: Apple creds (secrets). .env: MUSIC_SYNC_BUCKET public URL.
for f in "$ROOT/.env.build" "$ROOT/.env"; do
  if [[ -f "$f" ]]; then set -a; source "$f"; set +a; fi
done
# Alias the tide_share credential names so its .env.build can be copied as-is.
export APPLE_PASSWORD="${APPLE_PASSWORD:-${APPLE_APP_PASSWORD:-}}"
export APPLE_TEAM_ID="${APPLE_TEAM_ID:-${APPLE_NOTARY_TEAM_ID:-}}"

PUBLIC_BASE="${MUSIC_SYNC_BUCKET:-}"
# "bucket/prefix" — the app namespace inside the shared storage bucket.
R2_BUCKET="${MUSIC_SYNC_R2_BUCKET:-storage/music_sync}"
R2_ENDPOINT="${MUSIC_SYNC_R2_ENDPOINT:-https://d76960bd74940e6213de6aa16f3e04fd.r2.cloudflarestorage.com}"
R2_PROFILE="${MUSIC_SYNC_R2_PROFILE:-r2}"
KEY_FILE="$HOME/.tauri/musicsync.key"
KEY_PASSWORD="musicsync"

AWS=(aws --endpoint-url "$R2_ENDPOINT" --profile "$R2_PROFILE")

# ---- preflight ---------------------------------------------------------------
log "Preflight"
command -v python3 >/dev/null || die "python3 not found"
command -v pnpm >/dev/null || die "pnpm not found"
[[ -f "$TAURI_CONF" ]] || die "tauri.conf.json not found at $TAURI_CONF"
[[ -f "$MERGE_MANIFEST" ]] || die "scripts/merge_manifest.py not found at $MERGE_MANIFEST"
[[ -f "$KEY_FILE" ]] || die "updater signing key missing: $KEY_FILE
  Generate once with: pnpm tauri signer generate -w ~/.tauri/musicsync.key --password musicsync"
[[ -n "$PUBLIC_BASE" ]] || die "MUSIC_SYNC_BUCKET not set (public r2.dev base URL — see .env.example)"

# The updater pubkey must be the real one. A blank/placeholder pubkey ships an
# app that can never verify an update: it would download the artifact, fail
# signature verification and re-offer the same update forever.
PUBKEY="$(python3 -c "import json;print(json.load(open('$TAURI_CONF')).get('plugins',{}).get('updater',{}).get('pubkey',''))")"
if [[ -z "$PUBKEY" || "$PUBKEY" == "REPLACE_WITH_MUSICSYNC_PUBKEY" ]]; then
  die "plugins.updater.pubkey in $TAURI_CONF is empty or still the placeholder.
  Generate the keypair and paste its PUBLIC key there:
    pnpm tauri signer generate -w ~/.tauri/musicsync.key --password musicsync
  (the command prints the public key; it is also written to ~/.tauri/musicsync.key.pub)"
fi

if [[ $SKIP_UPLOAD -eq 0 ]]; then
  command -v aws >/dev/null || die "aws CLI not found (brew install awscli)"
  # Check the bucket root ("storage"), not the prefix — `s3 ls` on an empty
  # prefix exits 1, which would fail the very first release.
  "${AWS[@]}" s3 ls "s3://${R2_BUCKET%%/*}" >/dev/null 2>&1 || die "cannot access s3://${R2_BUCKET%%/*} with profile '$R2_PROFILE'.
  The R2 API token must include the '${R2_BUCKET%%/*}' bucket (Cloudflare →
  R2 → Manage API Tokens), or set MUSIC_SYNC_R2_PROFILE."
fi
if [[ $SKIP_NOTARIZE -eq 0 ]]; then
  [[ -n "${APPLE_ID:-}" && -n "${APPLE_PASSWORD:-}" && -n "${APPLE_TEAM_ID:-}" ]] || \
    die "Apple credentials missing (APPLE_ID / APPLE_PASSWORD / APPLE_TEAM_ID).
  Copy tide_share's .env.build to $ROOT/.env.build (see .env.build.example),
  or pass --skip-notarize for a local unsigned build."
  # Tauri does NOT auto-detect the identity — without APPLE_SIGNING_IDENTITY it
  # silently ad-hoc-signs and skips notarization. Detect it like tide_share does.
  if [[ -z "${APPLE_SIGNING_IDENTITY:-}" ]]; then
    APPLE_SIGNING_IDENTITY="$(security find-identity -v -p codesigning \
      | sed -n 's/.*"\(Developer ID Application: [^"]*\)".*/\1/p' | head -1)"
    [[ -n "$APPLE_SIGNING_IDENTITY" ]] || \
      die "no 'Developer ID Application' identity in the keychain (and APPLE_SIGNING_IDENTITY unset)"
  fi
  log "Signing identity: $APPLE_SIGNING_IDENTITY"
fi

# ---- version ----------------------------------------------------------------
# tauri.conf.json is authoritative (it names the bundle + the updater manifest).
# The other manifests must agree: they drift silently otherwise, which makes
# "what version is this?" unanswerable from the repo. Fail the release rather
# than publish ambiguity.
VERSION="$(python3 -c "import json;print(json.load(open('$TAURI_CONF'))['version'])")"
check_version_sync() {
  local mismatch=0 v
  v="$(python3 -c "import json;print(json.load(open('$ROOT/package.json')).get('version',''))")"
  [[ "$v" == "$VERSION" ]] || { echo "  package.json: ${v:-<missing>} (expected $VERSION)" >&2; mismatch=1; }
  # First `version = "…"` line in [package] — position-independent, unlike a
  # fixed line number which silently reads the wrong key after any edit.
  v="$(grep -m1 -E '^version = "' "$ROOT/src-tauri/Cargo.toml" | head -1 | sed -E 's/^version = "(.*)"/\1/')"
  [[ "$v" == "$VERSION" ]] || { echo "  src-tauri/Cargo.toml: ${v:-<missing>} (expected $VERSION)" >&2; mismatch=1; }
  if [[ $mismatch -eq 1 ]]; then
    echo "Version drift — bump every manifest to $VERSION before releasing." >&2
    exit 1
  fi
}
check_version_sync
ARCH="$(uname -m)"; [[ "$ARCH" == "arm64" ]] && ARCH="aarch64"
PLATFORM_KEY="darwin-$ARCH"
# Fixed names — the DMG link is permanent (shareable), the updater tar is
# arch-suffixed so a darwin-x86_64 build could coexist later.
TAR_NAME="MusicSync_${ARCH}.app.tar.gz"
DMG_NAME="MusicSync.dmg"
log "Version $VERSION · $PLATFORM_KEY"

REMOTE_MANIFEST="$(curl -fsS --max-time 15 "$PUBLIC_BASE/latest.json" 2>/dev/null || true)"
if [[ -n "$REMOTE_MANIFEST" ]]; then
  REMOTE_VERSION="$(printf '%s' "$REMOTE_MANIFEST" | python3 -c "import json,sys;print(json.load(sys.stdin).get('version',''))" 2>/dev/null || true)"
  if [[ "$REMOTE_VERSION" == "$VERSION" && $FORCE -eq 0 ]]; then
    die "v$VERSION is already published ($PUBLIC_BASE/latest.json).
  Bump the version in src-tauri/tauri.conf.json (+ package.json/Cargo.toml) or pass --force."
  fi
  log "Remote is v${REMOTE_VERSION:-none} — publishing v$VERSION"
else
  log "No remote latest.json yet — first release"
fi

if [[ $DRY_RUN -eq 1 ]]; then
  log "Dry run — would: build v$VERSION, sign+notarize$( [[ $SKIP_NOTARIZE -eq 1 ]] && echo ' (SKIPPED)' ), then upload (overwriting in place + pruning extras):"
  echo "    s3://$R2_BUCKET/mac/$TAR_NAME"
  echo "    s3://$R2_BUCKET/mac/$DMG_NAME   ← fixed download link"
  echo "    s3://$R2_BUCKET/latest.json  (platforms.$PLATFORM_KEY → $PUBLIC_BASE/mac/$TAR_NAME)"
  exit 0
fi

# ---- build ------------------------------------------------------------------
if [[ $MANIFEST_ONLY -eq 0 ]]; then
  log "Building (frontend → Tauri bundle)"
  (cd "$ROOT" && pnpm install)
  export TAURI_SIGNING_PRIVATE_KEY="$(cat "$KEY_FILE")"
  export TAURI_SIGNING_PRIVATE_KEY_PASSWORD="$KEY_PASSWORD"
  if [[ $SKIP_NOTARIZE -eq 1 ]]; then
    # Without Apple env vars Tauri neither signs nor notarizes — dev build.
    unset APPLE_ID APPLE_PASSWORD APPLE_TEAM_ID APPLE_SIGNING_IDENTITY 2>/dev/null || true
    log "Notarization SKIPPED — do not ship this build"
  else
    # Tauri v2 signs every binary + the .app, notarizes and staples natively
    # when these are set. No manual codesign pass needed (no sidecar).
    export APPLE_ID APPLE_PASSWORD APPLE_TEAM_ID APPLE_SIGNING_IDENTITY
  fi
  (cd "$ROOT" && pnpm tauri build)
fi

# Never publish an ad-hoc-signed build: this is exactly what happens when the
# signing env silently doesn't reach Tauri, and Gatekeeper would reject it.
if [[ $SKIP_NOTARIZE -eq 0 ]]; then
  if codesign -dv "$APP_BUNDLE" 2>&1 | grep -q "Signature=adhoc"; then
    die "built app is only ad-hoc signed — signing env did not reach Tauri; not publishing"
  fi
fi

# ---- artifact discovery -----------------------------------------------------
log "Collecting artifacts"
TAR_SRC="$(ls -t "$BUNDLE_DIR/macos/"*.app.tar.gz 2>/dev/null | head -1 || true)"
[[ -n "$TAR_SRC" ]] || die "no .app.tar.gz in $BUNDLE_DIR/macos (createUpdaterArtifacts?)"
SIG_SRC="$TAR_SRC.sig"
[[ -f "$SIG_SRC" ]] || die "missing updater signature: $SIG_SRC"
DMG_SRC="$(ls -t "$BUNDLE_DIR/dmg/"*.dmg 2>/dev/null | head -1 || true)"
[[ -n "$DMG_SRC" ]] || die "no .dmg in $BUNDLE_DIR/dmg"
SIGNATURE="$(tr -d '\n' < "$SIG_SRC")"
echo "    updater: $(basename "$TAR_SRC")  → mac/$TAR_NAME"
echo "    dmg:     $(basename "$DMG_SRC")  → mac/$DMG_NAME"

# ---- stale-bundle guard -----------------------------------------------------
# target/ survives between builds, so a failed/interrupted build can leave an
# OLDER bundle in place while the manifest advertises the NEW version. The
# updater would then install the old app, see the new version again on next
# launch, and loop forever. Two independent checks, both run in --manifest-only
# mode too (that mode publishes whatever is already on disk).
log "Verifying built artifacts match v$VERSION"
if [[ -d "$APP_BUNDLE" ]]; then
  APP_VERSION="$(/usr/libexec/PlistBuddy -c "Print :CFBundleShortVersionString" \
    "$APP_BUNDLE/Contents/Info.plist" 2>/dev/null || true)"
  [[ "$APP_VERSION" == "$VERSION" ]] || die "built app is v${APP_VERSION:-unknown} but the release is v$VERSION —
  stale bundle in $BUNDLE_DIR. Run: rm -rf src-tauri/target/release/bundle && rebuild."
else
  die "no app bundle at $APP_BUNDLE"
fi
# The signature's trusted comment records the file it was made for — the only
# self-describing link between the bytes we upload and the version we advertise.
SIG_FILE_NAME="$(python3 - "$SIG_SRC" <<'PY'
import base64, re, sys
raw = base64.b64decode(open(sys.argv[1]).read().strip()).decode("utf-8", "replace")
m = re.search(r"file:(\S+)", raw)
print(m.group(1) if m else "")
PY
)"
[[ "$SIG_FILE_NAME" == "$(basename "$TAR_SRC")" ]] || \
  die "updater signature is for '$SIG_FILE_NAME' but the artifact is '$(basename "$TAR_SRC")' —
  stale .sig next to a rebuilt tarball; refusing to publish."
echo "    app v$APP_VERSION · signature for $SIG_FILE_NAME"

# ---- manifest ---------------------------------------------------------------
log "Writing latest.json (mutating only platforms.$PLATFORM_KEY)"
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
printf '%s' "$REMOTE_MANIFEST" > "$STAGE/remote.json"
MANIFEST="$STAGE/latest.json"
# merge_manifest.py refuses to lower the global manifest version (exit 2) —
# do not swallow that.
python3 "$MERGE_MANIFEST" \
  --manifest "$STAGE/remote.json" \
  --out "$MANIFEST" \
  --platform "$PLATFORM_KEY" \
  --version "$VERSION" \
  --signature "$SIGNATURE" \
  --url "$PUBLIC_BASE/mac/$TAR_NAME"

# ---- upload -----------------------------------------------------------------
if [[ $SKIP_UPLOAD -eq 1 ]]; then
  log "Upload SKIPPED — manifest staged at $MANIFEST"
  trap - EXIT # keep the staged manifest around for inspection
else
  log "Uploading to s3://$R2_BUCKET (fixed names, overwrite in place)"
  # Artifacts BEFORE the manifest: latest.json is the trigger every installed
  # app polls, so it must never point at bytes that aren't up yet.
  # no-cache: fixed names get overwritten each release, so the r2.dev edge
  # must revalidate instead of serving a stale binary.
  "${AWS[@]}" s3 cp "$TAR_SRC" "s3://$R2_BUCKET/mac/$TAR_NAME" --cache-control "no-cache"
  "${AWS[@]}" s3 cp "$DMG_SRC" "s3://$R2_BUCKET/mac/$DMG_NAME" --cache-control "no-cache"
  "${AWS[@]}" s3 cp "$MANIFEST" "s3://$R2_BUCKET/latest.json" \
    --content-type application/json --cache-control "no-cache"

  # Host ONLY the latest release: prune anything under mac/ that isn't one of
  # the two fixed-name artifacts (e.g. leftovers from older naming schemes).
  log "Pruning mac/ (keeping $TAR_NAME + $DMG_NAME)"
  ("${AWS[@]}" s3 ls "s3://$R2_BUCKET/mac/" || true) | awk '{print $NF}' | while read -r obj; do
    [[ -z "$obj" || "$obj" == "$TAR_NAME" || "$obj" == "$DMG_NAME" ]] && continue
    log "  rm mac/$obj"
    "${AWS[@]}" s3 rm "s3://$R2_BUCKET/mac/$obj"
  done

  log "Published: $PUBLIC_BASE/latest.json"
  log "Fixed download link: $PUBLIC_BASE/mac/$DMG_NAME"

  # ---- post-publish verification --------------------------------------------
  # Read back what the world actually sees. A green release that published the
  # wrong bytes must fail here, not on a user's machine.
  log "Verifying published manifest"
  sleep 3
  curl -fsS --max-time 20 "$PUBLIC_BASE/latest.json" -o "$STAGE/published.json" \
    || die "cannot re-read $PUBLIC_BASE/latest.json"
  PUBLISHED_FILE="$STAGE/published.json" VERSION="$VERSION" PLATFORM_KEY="$PLATFORM_KEY" \
  EXPECTED_URL="$PUBLIC_BASE/mac/$TAR_NAME" python3 <<'PY' || die "published manifest is wrong — investigate before shipping"
import json, os, sys

data = json.load(open(os.environ["PUBLISHED_FILE"]))
want_version = os.environ["VERSION"]
key = os.environ["PLATFORM_KEY"]
if data.get("version") != want_version:
    sys.exit(f"ERROR: published manifest says {data.get('version')!r}, expected {want_version!r}")
entry = (data.get("platforms") or {}).get(key)
if not isinstance(entry, dict):
    sys.exit(f"ERROR: published manifest has no platforms.{key} entry")
if not entry.get("signature"):
    sys.exit(f"ERROR: platforms.{key} has an empty signature")
if entry.get("url") != os.environ["EXPECTED_URL"]:
    sys.exit(f"ERROR: platforms.{key} url is {entry.get('url')!r}, expected {os.environ['EXPECTED_URL']!r}")
print(f"OK: manifest {want_version} · {key} → {entry['url']}")
PY
  curl -fsIL --max-time 20 "$PUBLIC_BASE/mac/$TAR_NAME" >/dev/null \
    || die "updater artifact not reachable at $PUBLIC_BASE/mac/$TAR_NAME"
  curl -fsIL --max-time 20 "$PUBLIC_BASE/mac/$DMG_NAME" >/dev/null \
    || die "DMG not reachable at $PUBLIC_BASE/mac/$DMG_NAME"
  log "Updater verified: $PLATFORM_KEY v$VERSION"
fi

# ---- tag --------------------------------------------------------------------
if git -C "$ROOT" rev-parse "v$VERSION" >/dev/null 2>&1; then
  log "Tag v$VERSION already exists — leaving it"
else
  git -C "$ROOT" tag -a "v$VERSION" -m "MusicSync v$VERSION"
  log "Tagged v$VERSION (not pushed)"
fi

log "Done — v$VERSION"
