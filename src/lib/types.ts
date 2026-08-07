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
  ip: string;
  alias: string | null;
  last_seen: number | null;
  pinned_manual: boolean;
}

/**
 * Group role (FR-9/FR-13), derived on the Rust side from `getStatusEx` +
 * `multiroom:getSlaveList`. Discriminated on `kind`.
 */
export type DeviceRole =
  | { kind: "solo" }
  | { kind: "master"; slaveUuids: string[] }
  | { kind: "slave"; masterUuid: string | null; masterIp: string | null };

/**
 * Active input (FR-19). The listed values are the codes we map; anything else
 * arrives as `"mode <n>"`, so the union stays open on purpose.
 */
export type SourceMode =
  | "idle"
  | "airplay"
  | "dlna"
  | "network"
  | "usb"
  | "spotify"
  | "line-in"
  | "bluetooth"
  | "optical"
  | "line-in2"
  | "usb-dac"
  | "follower"
  | (string & {});

/** Playback state from `getPlayerStatus` (FR-18). */
export interface PlayerInfo {
  /** `play` | `pause` | `stop` | `load`. */
  status: string;
  /** Raw Linkplay mode code, kept for the debug pane. */
  mode: number;
  source: SourceMode;
  vol: number;
  mute: boolean;
  /** Position/length in ms. */
  curpos: number;
  totlen: number;
  title: string;
  artist: string;
  album: string;
}

/** A group member as reported by the master. */
export interface SlaveInfo {
  uuid: string;
  name: string;
  ip: string;
  volume: number;
  mute: boolean;
}

/** Live per-device state — the payload of `device-updated`/`device-offline`. */
export interface DeviceSnapshot {
  uuid: string;
  ip: string;
  /** Name reported by the device itself. */
  name: string;
  /** Local override (FR-7). */
  alias: string | null;
  /** Alias, else device name, else IP — what the UI prints. */
  displayName: string;
  online: boolean;
  role: DeviceRole;
  groupName: string;
  firmware: string;
  hardware: string;
  project: string;
  mcuVer: string;
  rssi: number | null;
  ssid: string;
  /** Absent while the device is a follower, idle-unreadable, or offline. */
  player: PlayerInfo | null;
  slaves: SlaveInfo[];
  /** Unix ms of the last successful poll. */
  lastSeen: number | null;
}

/** Device detail view (FR-9), including the unmodelled raw fields. */
export interface DeviceDetail {
  snapshot: DeviceSnapshot;
  /** `getStatusEx` keys we don't model — the debug pane. */
  extra: Record<string, unknown>;
  /** `getPlayerStatus` keys we don't model. */
  playerExtra: Record<string, unknown>;
}

/** User preferences (FR-20 / FR-27), persisted in settings.json. */
export interface Settings {
  poll_ms: number;
  subnet: string | null;
  theme: string;
  guard_mode: "ask" | "always" | "never";
  failover_mode: "prompt" | "auto" | "never";
  start_at_login: boolean;
}
