markdown
Copy
# BRIEF.md — LP10 LAN Multiroom Controller

## 1. Overview

A cross-platform desktop app (Windows/macOS/Linux) to discover, group, and control
Arylic LP10 streamers (Linkplay-based) on the local network. It replaces the need for
Google Cast / GO CONTROL for multiroom: grouping and sync are handled natively by the
Linkplay firmware; the app is a LAN orchestrator over the Linkplay HTTP API.

**Goal:** one-click "play the same music everywhere" — group all LP10s, control volume,
then stream to the master via Spotify Connect / AirPlay / DLNA.

**Key behavioral fact (Spotify Connect):** grouped LP10s do NOT appear as a merged
entity in Spotify Connect. Each unit is listed individually; selecting the **master**
plays synced audio on all slaves. Selecting a **slave** can pull it out of the group.
The app must make the master obvious and actively protect group integrity (see 4.3.1).

## 2. Tech Stack
Layer	Choice	Notes
Shell	Tauri 2.x	Rust backend, small footprint
Frontend	SvelteKit (Svelte 5) + TypeScript	SPA mode (adapter-static), SSR disabled
Styling	Tailwind CSS	fast iteration; dark mode default
HTTP client (Rust)	reqwest	calls to http://<ip>/httpapi.asp
Discovery (Rust)	mdns-sd + ssdp-client; fallback: subnet scan	LP10 answers UPnP/DLNA + mDNS
State (frontend)	Svelte stores/runes	device list, groups, volumes
Persistence	Tauri store plugin (JSON)	devices, settings, auth hash
Packaging	Tauri bundler	msi/dmg/AppImage
3. Core Concepts (Linkplay API)
Every device exposes GET http://<ip>/httpapi.asp?command=<cmd> (port 80, no auth).
Key commands:
getStatusEx — device info (name, UUID, firmware, group role)
getPlayerStatus — playback state, volume, source, track
ConnectMasterAp:JoinGroupMaster:eth<master_ip>:wifi0.0.0.0 — join group (sent to slave)
multiroom:Ungroup — leave/dissolve group
multiroom:SlaveVolume:<ip>:<vol> — per-slave volume via master
setPlayerCmd:vol:<0-100>, setPlayerCmd:pause, setPlayerCmd:resume, setPlayerCmd:next/prev
Reference implementations: python-linkplay (GitHub), Home Assistant linkplay integration.
3.1 Spotify Connect interaction model
Spotify Connect discovers each LP10 individually; there is no merged "group" endpoint.
The master is the correct target: audio selected on it plays on all grouped slaves in sync.
Selecting a slave directly in Spotify Connect (or AirPlay/DLNA to a slave) may detach
it from the group ("source hijack"). This is a firmware behavior we must design around.
4. Functional Requirements
4.1 First-run / Auth
FR-1: On first launch, show a setup wizard: create a local profile (username + password
or PIN). Stored locally as an Argon2 hash (no cloud, no account server).
FR-2: Subsequent launches require login; optional "remember me" (skip login on this machine).
FR-3: Settings page allows changing/removing the password.
4.2 Device Discovery & Management
FR-4: "Scan devices" button — mDNS/SSDP scan + optional subnet sweep (CIDR configurable,
default auto-detected, e.g. 192.168.10.0/24). Show progress.
FR-5: "Add device manually" — by IP; validate via getStatusEx.
FR-6: Device list persisted; on startup, re-poll known devices (online/offline badge).
FR-7: Rename device (friendly name, local alias; optionally push to device).
FR-8: Remove device from list.
FR-9: Device detail view: IP, UUID, firmware version, RSSI, group role (master/slave/solo).
4.3 Grouping / Sync
FR-10: "Sync all" — pick a master (first online device or user choice) and join all
others to it with one click.
FR-11: Custom groups: drag or checkbox devices into a group; choose master.
FR-12: "Ungroup all" and per-device leave-group.
FR-13: Show current topology clearly (master ⇒ slaves).
FR-14: Handle join failures with retry (up to 3, backoff) and per-device error surface.
4.3.1 Spotify Connect / group-integrity handling
FR-22 (Master naming): When a group is created, offer to push a distinctive name to
the master device (e.g. "Whole House 🔊") and optionally prefix slaves (e.g. "(grouped)
Kitchen"), so the correct target is obvious in Spotify Connect. Names restored on ungroup.
FR-23 (Verification spike): During M2, empirically verify on real LP10 firmware:
(a) master appears in Spotify Connect and plays to all slaves in sync;
(b) what exactly happens when a slave is selected directly (detach? full group break?);
(c) whether the slave's Connect entry is suppressed/hidden while grouped.
Findings recorded in docs/firmware-notes.md and drive FR-24 behavior.
FR-24 (Auto-heal / group guard): A "Group Guard" toggle (default ON while a group is
active). The poller monitors each device's group role via getStatusEx; if a slave
detaches (source hijack or reboot), the app:
detects the change within one polling cycle,
shows a non-blocking notification ("Kitchen left the group — rejoining…"),
auto-rejoins it to the master (respecting FR-14 retry/backoff),
if the detach was caused by an active local playback on that unit, asks the user
instead of force-rejoining (configurable: ask / always rejoin / never).
FR-25 (Master offline failover): If the master goes offline, prompt to promote a
slave to master and rebuild the group (optionally automatic, configurable).
FR-26 (UI clarity): Dashboard must always show a prominent "▶ Select
in Spotify Connect" hint while a group is active, with a master badge on the device card.
4.4 Playback & Volume
FR-15: Master volume slider (applies proportionally to all group members).
FR-16: Per-device volume sliders + mute toggles.
FR-17: Transport controls on the group/master: play/pause, next/prev (works when a
source like Spotify Connect/DLNA is active).
FR-18: Now-playing display (title/artist/source) polled from getPlayerStatus
every 2–5 s (adaptive: faster when window focused).
FR-19: Show active input/source per device (Wi-Fi stream, AirPlay, line-in, etc.).
4.5 Settings
FR-20: Polling interval, subnet for scan, theme, start-at-login (optional).
FR-21: Export/import config (JSON).
FR-27: Group Guard behavior (ask / always rejoin / never), master-failover mode,
group naming template.
5. Non-Functional Requirements
NFR-1: LAN-only; no external services. App must work fully offline.
NFR-2: Command latency target < 300 ms on LAN; UI optimistic updates with rollback.
NFR-3: Resilience: devices going offline must not freeze UI; polling is per-device
with timeouts (~2 s) and independent failure handling.
NFR-4: All device HTTP calls from the Rust side (avoid CORS/mixed-content issues).
NFR-5: Binary < 20 MB; idle CPU near 0%; no telemetry.
NFR-6: Codebase: typed API layer (Rust enums for commands, serde models for responses).
NFR-7: Group Guard detection latency ≤ 1 polling cycle (default 3 s while grouped).
6. UI Sketch (pages)
Login / First-run wizard
Dashboard — device cards (name, status, role badge, volume), "Sync all",
"Ungroup all", master volume, now playing, Spotify Connect hint (FR-26),
Group Guard status/notifications
Devices — scan, add manually, rename, remove, detail view
Groups — build custom groups, pick master, group naming (FR-22)
Settings — auth, polling, subnet, theme, Group Guard & failover (FR-27), import/export
7. Architecture
SvelteKit UI  ──invoke()──▶  Tauri (Rust)
  stores/runes                ├─ discovery.rs   (mdns/ssdp/subnet scan)
  optimistic UI               ├─ linkplay.rs    (typed HTTP API client)
  event listeners ◀─emit()──  ├─ poller.rs      (status loop, emits events)
                              ├─ guard.rs       (group integrity monitor + auto-heal)
                              └─ store.rs       (persisted config, auth)
Rust emits device-updated, device-offline, scan-progress, group-changed,
group-healed, group-heal-failed events; UI subscribes.
Commands (invoke): scan, add_device, join_group, ungroup, set_volume,
player_cmd, get_status, login, set_password, set_guard_mode, promote_master.
8. Milestones
#	Deliverable	Est.
M1	Scaffold (Tauri+SvelteKit+Tailwind), login/first-run	0.5–1 d
M2	Linkplay client + manual add + device list/status + Spotify Connect verification spike (FR-23)	1–1.5 d
M3	Discovery (mDNS/SSDP + subnet scan)	1 d
M4	Grouping (sync all, custom groups, ungroup) + master naming (FR-22)	1 d
M5	Volume + transport + now playing polling + Group Guard (FR-24/25)	1.5 d
M6	Settings, persistence, packaging, polish	1–2 d
9. Risks & Notes
Linkplay API is undocumented/unofficial; firmware differences may alter responses —
keep the API layer tolerant (unknown fields ignored, fallbacks).
Group join command syntax varies slightly across firmware generations; test against
actual LP10 firmware early (M2 spike: run getStatusEx + a join/ungroup cycle).
Spotify Connect behavior toward grouped units is firmware-dependent — exact
detach semantics must be confirmed in the FR-23 spike before finalizing Group Guard
defaults. Do not assume slave entries are hidden while grouped.
Auto-rejoin loops: if a user deliberately plays to a slave, naive auto-heal would
fight them — hence the "ask" default when local playback is detected (FR-24.4).
Spotify playback itself stays on Spotify Connect/AirPlay to the master; this app does
not decode or relay audio.
10. Out of Scope (v1)
Audio streaming/decoding in-app, EQ, presets, alarms, firmware updates,
mobile builds (Tauri mobile possible later), multi-user roles.

Changes: new §3.1 (Spotify Connect model), §4.3.1 with FR-22–FR-26 (master naming, verification spike, Group Guard auto-heal, master failover, UI hint), FR-27 settings, NFR-7, `guard.rs` in architecture, updated milestones and risks.