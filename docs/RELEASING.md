# Releasing MusicSync

Self-hosted distribution under the **`music_sync/` prefix of the shared house `storage`
R2 bucket** (same bucket and `r2` API token as tide_reel/tide_share/tide_pos2),
following the house pipeline. macOS builds run locally via
`scripts/build_release.sh`; Windows builds run in GitHub Actions.

The repo is a single flat package: `src/` (SvelteKit) + `src-tauri/` (Rust crate
`music-sync`, productName `MusicSync`) at the root.

## Bucket layout — only the latest release, fixed links

```
s3://storage/music_sync/latest.json                      ← ONE updater manifest, all platforms
s3://storage/music_sync/mac/MusicSync.dmg                ← installer — PERMANENT download link
s3://storage/music_sync/mac/MusicSync_aarch64.app.tar.gz ← updater artifact (signed, fixed name)
s3://storage/music_sync/windows/MusicSync-setup.exe      ← installer — PERMANENT download link
```

- Public base URL: `MUSIC_SYNC_BUCKET` in `.env`
  (`https://pub-7b3e5bbd605d43adbeb4601a962d84bd.r2.dev/music_sync`). The updater
  endpoint baked into `src-tauri/tauri.conf.json` is `{MUSIC_SYNC_BUCKET}/latest.json`.
- **Fixed names, overwrite in place**: the download link
  `{MUSIC_SYNC_BUCKET}/mac/MusicSync.dmg` never changes — share it once. Every upload
  uses `Cache-Control: no-cache` so the r2.dev edge revalidates instead of serving a
  stale binary, and each release script **prunes** everything else under its platform
  prefix so only the latest version is ever hosted.
- `latest.json` routes platforms internally (`platforms["darwin-aarch64"]`,
  `platforms["windows-x86_64"]`). Each platform's release path calls the *same*
  `scripts/merge_manifest.py`, which mutates **only its own key** — mac and windows
  releases never clobber each other.

## One-time setup (per machine)

1. **Updater keypair**: private key `~/.tauri/musicsync.key` (password `musicsync`,
   house convention), public key pasted into
   `src-tauri/tauri.conf.json → plugins.updater.pubkey`. Generate once:

   ```bash
   pnpm tauri signer generate -w ~/.tauri/musicsync.key --password musicsync
   ```

   The command prints the **public** key (also written to `~/.tauri/musicsync.key.pub`) —
   paste that into `plugins.updater.pubkey`. Regenerate only if lost: a new key orphans
   every installed app (they can no longer verify updates and must be reinstalled by hand).
2. **R2 access**: AWS CLI with the house `r2` profile (`aws configure --profile r2`) —
   the same token tide_reel/tide_share already use covers the `storage` bucket; no extra
   token needed. `build_release.sh` preflights access.
3. **Apple credentials**: copy `.env.build.example` → `.env.build` (gitignored) and fill
   `APPLE_ID` / `APPLE_PASSWORD` (app-specific) / `APPLE_TEAM_ID` — same values as
   tide_share's `.env.build` (its `APPLE_APP_PASSWORD` / `APPLE_NOTARY_TEAM_ID` names are
   accepted too, the script aliases them). Signing identity is auto-detected from the
   keychain (`security find-identity`) unless `APPLE_SIGNING_IDENTITY` is set.
4. **GitHub secrets** for the Windows workflow — from this Mac with the `gh` CLI:

   ```bash
   gh secret set TAURI_SIGNING_PRIVATE_KEY < ~/.tauri/musicsync.key
   gh secret set TAURI_SIGNING_PRIVATE_KEY_PASSWORD --body musicsync
   gh secret set R2_ACCESS_KEY_ID --body '<r2 access key id>'
   gh secret set R2_SECRET_ACCESS_KEY --body '<r2 secret access key>'
   ```

## Release procedure

1. **Bump the version in all three files** — they must agree or the script aborts:
   - `src-tauri/tauri.conf.json` (**authoritative** — it names the bundle and the manifest)
   - `package.json`
   - `src-tauri/Cargo.toml`
2. **Commit and push** the bump. Pushing *before* the Windows run matters: CI builds from
   the pushed ref, and a stale checkout would try to downgrade the shared manifest.
3. **macOS**: `./scripts/build_release.sh`
   preflight → remote-version guard → `pnpm install` → `pnpm tauri build` (Tauri v2 signs,
   notarizes and staples natively) → ad-hoc + stale-bundle guards → manifest merge →
   upload artifacts then manifest → prune `mac/` → post-publish verification → `git tag -a v<version>`.
4. **Windows**: `gh workflow run release-windows` (or Actions → *release-windows* → *Run
   workflow*). ~15 min on the runner. **Same version as the mac release.**
5. Verify: `curl $MUSIC_SYNC_BUCKET/latest.json` shows the new version with both platform
   keys; an older install shows the update banner on next launch.

Flags: `--dry-run` (plan only) · `--skip-notarize` (local dev build, never ship) ·
`--skip-upload` (inspect staged manifest) · `--manifest-only` (re-publish from the newest
already-built artifacts) · `--force` (republish a version already live).

## Guards — why the script refuses things

Every one of these exists because of a real incident. Do not "just bypass" them.

- **Pubkey placeholder guard** — the release aborts if
  `plugins.updater.pubkey` is empty or still `REPLACE_WITH_MUSICSYNC_PUBKEY`. An app
  shipped with no real pubkey downloads updates it can never verify and re-offers the
  same update forever.
- **Version-sync gate** — `tauri.conf.json`, `package.json` and `src-tauri/Cargo.toml`
  must all carry the same version (Cargo's is read as the first `version = "…"` line, not
  a fixed line number). Drift makes "what version is this?" unanswerable from the repo.
- **Remote-republish guard** — refuses to publish a version already live unless `--force`.
- **Ad-hoc signature guard** — if `codesign -dv` reports `Signature=adhoc`, the signing
  env never reached Tauri; Gatekeeper would reject the build, so it is not published.
- **Stale-bundle guard** (mac and windows) — `target/` survives between builds, so an
  interrupted build can leave an *older* bundle while the manifest advertises the *new*
  version. Two checks: the built `.app`'s `CFBundleShortVersionString` must equal the
  release version, and the `.sig` trusted comment (`file:<name>`) must name the exact
  artifact being uploaded. Windows additionally demands
  `MusicSync_${VERSION}_x64-setup.exe` **by name** — never "newest wins" — and CI purges
  `src-tauri/target/release/bundle` before building.
- **Downgrade guard** (`scripts/merge_manifest.py`, exit 2) — a merge that would *lower*
  the global manifest version is refused. Use `--allow-downgrade` only for a deliberate
  rollback.
- **Post-publish verification** — after upload both scripts re-read the *public*
  `latest.json`, assert the version and a complete platform entry, and `HEAD` the public
  artifact URLs. A release that published the wrong bytes fails here, not on a user's machine.

## Troubleshooting / lessons carried over

- **Update loop, Windows** — a cached `target/` let a stale `0.1.3` setup.exe be published
  under a `0.1.5` manifest. The updater saw a newer version, downloaded, verified (the old
  signature matched the old bytes), reinstalled the *old* app, and re-offered the update on
  every launch. Fixed by the exact-filename demand, the `.sig` trusted-comment assertion and
  the CI bundle purge.
- **Manifest downgrade, 2026-07-14** — a Windows CI run built from an un-pushed `main`
  (still `0.6.6`) and its manifest write landed 2 s after the mac `0.6.7` upload. It
  downgraded the global manifest *and* restored a stale mac signature that no longer matched
  the artifact at the fixed mac URL; installed apps stopped seeing updates. Fixed by the
  monotonic downgrade guard — and by pushing the bump before running CI.
- **Version lockstep** — `latest.json` carries ONE global `version`. After a bump, release
  **both** platforms; the lagging one will otherwise offer an update it cannot satisfy.
- **Stale edge cache** — if `latest.json` still shows the old version, check the upload used
  `--cache-control no-cache` (both scripts do) and wait a few seconds; the post-publish
  verification already sleeps 3 s before re-reading.
- **`s3 ls` on an empty prefix exits 1** — the preflight checks the bucket *root*
  (`storage`), not `storage/music_sync`, so the very first release does not fail.
- **No Windows code-signing certificate** — SmartScreen shows "unrecognized app" on first
  run. Buy an OV/EV cert to remove it.
- **Actions cost** — private-repo Actions are free up to 2,000 runner-minutes/month;
  Windows counts 2×, so one release ≈ 30 billed minutes.

## Linux (when we get there)

Same pattern on an `ubuntu-22.04` runner (AppImage/deb, `linux-x86_64` key — already in
`merge_manifest.py`'s `VALID_PLATFORMS` — under a `/linux` prefix): copy
`release-windows.yml` + `publish_windows.sh` and adjust the bundle paths/names.
