/** Shared types mirroring the Rust command surface (src-tauri/src). */

/** Boot-time routing answer from `auth_state` (brief.md FR-1 / FR-2). */
export interface AuthState {
  /** A local profile exists; otherwise the setup wizard runs. */
  configured: boolean;
  /** The login screen must clear before the app is usable. */
  requiresLogin: boolean;
  /** False after FR-3's "remove password": configured, but opens unprompted. */
  hasPassword: boolean;
  username: string | null;
  rememberMe: boolean;
}

/** The `{ code, message }` envelope every failed command rejects with. */
export interface AppErrorPayload {
  code: string;
  message: string;
}

/** Persisted device entry (FR-5/FR-6). The UUID is the identity, not the IP. */
export interface SavedDevice {
  uuid: string;
  usn: string;
  ip: string;
  alias: string | null;
  net_mode: string | null;
  last_seen: number | null;
  pinned_manual: boolean;
}

/** Group role, read from the DDMS `State` banner. R2 acts on it; R1 shows it. */
export type Role = "solo" | "master" | "slave";

/** Transport verbs accepted by `player_cmd` (R3). */
export type PlayerCmd = "play" | "pause" | "next" | "prev" | "stop";

/** Wired vs Wi-Fi, from the device's `DevInfo` MACs + DDMS `NETMODE`. */
export type NetMode = "ethernet" | "wifi";

/** Now-playing metadata (best-effort from `TRACK_INFO` / `GETPLAYDURATION`). */
export interface Track {
  title: string;
  artist: string;
  album: string;
  /** Total length in ms, when known. */
  durationMs: number | null;
  /** Current position in ms (from `GETPLAYDURATION` pushes), when known. */
  positionMs: number | null;
}

/** Live per-device state — the payload of `device-updated`/`device-offline`. */
export interface DeviceSnapshot {
  uuid: string;
  ip: string;
  /** Name reported by the device (DDMS `DeviceName` / `DevName`). */
  name: string;
  /** Local override (FR-7). */
  alias: string | null;
  /** Alias, else device name, else IP — what the UI prints. */
  displayName: string;
  online: boolean;
  /** `ethernet` | `wifi`, when known. */
  netMode: NetMode | null;
  /** `ETH` | `2G` | `5G`, verbatim. */
  wifiBand: string | null;
  model: string;
  firmware: string;
  role: Role;
  /** DDMS zone id when grouped (R2 fills this in fully). */
  groupId: string | null;
  /** The master this device follows, when a slave. */
  masterUuid: string | null;
  volume: number | null;
  mute: boolean;
  /** Raw `CURRSOURCE` integer. */
  source: number | null;
  /** Human label for `source` (`Idle` / `Streaming` / `Source N`). */
  sourceLabel: string | null;
  /** Raw `PLAY_STATE` integer (`0` stopped / `1` playing). */
  playState: number | null;
  track: Track | null;
  /** Unix ms of the last successful poll. */
  lastSeen: number | null;
  /** Verbatim raw Luci/DDMS payloads, for the debug pane. */
  raw: Record<string, unknown>;
}

/** Device detail view (FR-9), including the raw payloads. */
export interface DeviceDetail {
  snapshot: DeviceSnapshot;
  /** Raw Luci/DDMS payloads keyed by source — the debug pane. */
  raw: Record<string, unknown>;
}

// ------------------------------------------------------------- discovery --

/** Which discovery strategy a `scan-progress` event is reporting on (FR-4). */
export type ScanPhase = "ddms" | "ssdp" | "sweep";

/** `scan` arguments. `sweep` defaults on; `cidr` null = settings, then auto. */
export interface ScanOptions {
  sweep: boolean;
  cidr: string | null;
}

/**
 * A device found by a scan — confirmed via a DDMS banner or Luci `DevInfo`, but
 * *not* saved. Adding one goes through the normal `add_device` path (FR-5).
 */
export interface DeviceCandidate {
  /** UPnP UDN uuid — the stable key (may be empty when only Luci/DDMS saw it). */
  uuid: string;
  /** DDMS `USN` (a MAC), the fallback key. */
  usn: string;
  ip: string;
  /** The device's own name; candidates have no local alias yet. */
  name: string;
  model: string;
  firmware: string;
  netMode: NetMode | null;
  wifiBand: string | null;
  /** Already saved — matched on uuid, then usn, then ip. */
  alreadyAdded: boolean;
}

/** Progress for one strategy. `total` is 0 for mDNS/SSDP (no denominator). */
export interface ScanProgress {
  phase: ScanPhase;
  scanned: number;
  total: number;
  /** Candidates confirmed so far, across all strategies. */
  found: number;
}

/** The scan ended — normally, or because it was cancelled. */
export interface ScanComplete {
  found: number;
  cancelled: boolean;
}

/** UI theme. `system` follows the OS `prefers-color-scheme`. */
export type Theme = "dark" | "light" | "system";

/** User preferences (FR-20 / FR-27), persisted in settings.json. */
export interface Settings {
  /** Poll-interval floor in ms (the poller's adaptive cadence never beats it). */
  poll_ms: number;
  subnet: string | null;
  theme: Theme;
  /** Per-request network budget in ms. */
  http_timeout_ms: number;
  start_at_login: boolean;
  // Grouping is unsupported on the LP10 (firmware-notes §G/§H); these two are
  // kept only for on-disk back-compat and are never surfaced in the UI.
  guard_mode?: string;
  failover_mode?: string;
}

/** Partial settings update for `update_settings`; only changed fields are sent. */
export type SettingsPatch = Partial<
  Pick<Settings, "poll_ms" | "theme" | "http_timeout_ms" | "start_at_login">
>;
