# MusicSync — Project Guide

> **Standing rule:** [`journal.html`](journal.html) is the source of truth for progress. Re-read it at the start of every session; update it (phase board, commit log, decisions) as work lands. The full product spec is `brief.md` (FR/NFR numbers referenced everywhere).

## 1. What this is

A LAN-only Tauri desktop app (Win/mac/Linux) that discovers, groups, and controls **Arylic LP10** streamers (Linkplay firmware) over `GET http://<ip>/httpapi.asp?command=…` — a native replacement for GO CONTROL multiroom. Core value: one-click "sync all", make the Spotify Connect **master** unmistakable, and auto-heal groups when a slave detaches ("Group Guard", FR-24). No cloud, no telemetry, works fully offline (NFR-1).

## 2. Stack & layout

Flat single package (deliberately **no** monorepo — no shared-types consumer): SvelteKit 2 + Svelte 5 runes + Tailwind 4 (`@tailwindcss/vite`) + TS strict, SPA via `adapter-static`; Tauri 2 shell; pnpm (pinned via `packageManager`), Node ≥ 20.16 (toolchain pinned to Vite 6/TS 5.9 for Node 20.16 — Vite 8 needs ≥ 20.19).

```
src/lib/tauri/        typed invoke wrappers (commands.ts) + event listeners (events.ts) — pages never call invoke directly
src/lib/stores/       *.svelte.ts runes classes; idempotent start() called from +layout.svelte only
src/routes/           / (dashboard), setup, login, devices, groups, settings
src-tauri/src/        error.rs, state.rs, store.rs, net.rs, poller.rs, guard.rs, group.rs,
                      linkplay/{client,models,hexstr}.rs, commands/{auth,devices,groups,playback,settings}.rs
scripts/              build_release.sh (mac), publish_windows.sh, merge_manifest.py (shared, downgrade guard)
docs/                 RELEASING.md · firmware-notes.md (FR-23 hardware spike findings — gates guard defaults)
```

## 3. Architecture rules

- **All device HTTP lives in Rust** (NFR-4). The webview CSP has no LAN access; frontend talks only via `invoke`/events.
- **Firmware variance is the central risk.** Command syntax exists in exactly one place: `LinkplayCommand::to_query()`. Role derivation (master/slave/solo) is one function. Serde models are `#[serde(default)]` + `#[serde(flatten)]` extras, numbers parsed string-or-number (`de_num`), `Title/Artist/Album` hex-decoded with plain-text fallback. Never assume a field exists.
- **reqwest client**: 2 s timeout, **`.no_proxy()`** (system proxy must not swallow LAN calls), no TLS features (plain http only, keeps binary small — NFR-5).
- **Poller**: one tokio task per device — a hung device must never stall others (NFR-3). Emit events only on change.
- **Group Guard** must never fight the user: "ask" default on local-playback detach, flapping suppression, 5 s settling window after app-initiated group mutations.
- Event listeners registered once in store `start()` from the layout; pages only read stores.

## 4. Updater contract is sacred

Load-bearing for every installed copy — never change: `productName: "MusicSync"`, `identifier: "com.musicsync.app"`, the updater `pubkey` + endpoint in `tauri.conf.json`, the `latest.json` schema, and the keypair `~/.tauri/musicsync.key` (password `musicsync`; losing it orphans all installs). `latest.json` has ONE global version with per-platform keys: always release mac + Windows at the same version.

## 5. Build / dev / release

- Dev: `pnpm tauri:dev`. Quality gate: `pnpm check` + `pnpm build` + `cargo check --all-targets` + `cargo test --lib`.
- Release: bump version in **3 files** (`tauri.conf.json` authoritative, `package.json`, `src-tauri/Cargo.toml`) → commit → push → `./scripts/build_release.sh` (mac: sign, notarize, upload to R2, verify) → `gh workflow run release-windows`. Guards: version-sync, pubkey-placeholder, stale-bundle, adhoc-signature, monotonic-manifest (merge_manifest.py), post-publish verification. See `docs/RELEASING.md`.
- R2: `s3://storage/music_sync/` (aws profile `r2`), public `https://pub-7b3e5bbd605d43adbeb4601a962d84bd.r2.dev/music_sync`.

## 6. Security

Never commit: `.env`, `.env.build`, `*.key`. `.env.example` / `.env.build.example` document vars without values. Auth is a local argon2 hash only; config export strips it.

## 7. Reference map

- `../tide_pos2` — authoritative for SvelteKit/Tailwind/Tauri conventions and the release-script pattern this repo copied.
- `../tide_share/scripts/merge_manifest.py` — origin of the manifest downgrade guard (2026-07-14 incident note).
- `brief.md` §3 + python-linkplay / Home Assistant linkplay integration — Linkplay API reference. **Verify against real LP10 firmware (FR-23 spike) before hardening group/guard behavior**; findings go in `docs/firmware-notes.md`.

## 8. Git & commits

Conventional, milestone-scoped messages (`M2: …`). One buildable commit per logical unit; commit `pnpm-lock.yaml` + `Cargo.lock`. Log every commit in journal.html. End commit messages with:
`Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`
