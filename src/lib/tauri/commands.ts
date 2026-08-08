import { invoke } from "@tauri-apps/api/core";
import type {
  AppErrorPayload,
  AuthState,
  DeviceCandidate,
  DeviceDetail,
  DeviceSnapshot,
  PlayerCmd,
  ScanOptions,
  Settings,
  SettingsPatch,
  SpotifyState,
} from "$lib/types";

/**
 * Typed wrappers over the Rust auth commands. Keeping the `invoke` strings in
 * one place means a renamed command is a compile error here rather than a
 * runtime "command not found" in a component.
 */
export const commands = {
  authState: () => invoke<AuthState>("auth_state"),

  createProfile: (username: string, password: string) =>
    invoke<void>("create_profile", { username, password }),

  /** Resolves `false` on wrong credentials; rejects when throttled. */
  login: (username: string, password: string) =>
    invoke<boolean>("login", { username, password }),

  logout: () => invoke<void>("logout"),

  setPassword: (current: string, next: string) =>
    invoke<void>("set_password", { current, next }),

  removePassword: (current: string) => invoke<void>("remove_password", { current }),

  setRememberMe: (value: boolean) => invoke<void>("set_remember_me", { value }),

  // ------------------------------------------------------------- devices --

  /**
   * FR-5: confirm an IP over Luci (`DevInfo` + a DDMS M-SEARCH) and persist it.
   * Idempotent — re-adding a known device refreshes its IP instead of failing.
   */
  addDevice: (ip: string) => invoke<DeviceSnapshot>("add_device", { ip }),

  /** FR-8. */
  removeDevice: (uuid: string) => invoke<void>("remove_device", { uuid }),

  /** FR-7. `alias: null` restores the device's own name. */
  renameDevice: (uuid: string, alias: string | null, pushToDevice: boolean) =>
    invoke<DeviceSnapshot>("rename_device", { uuid, alias, pushToDevice }),

  /** FR-6: the persisted list, hydrated with the poller's latest state. */
  listDevices: () => invoke<DeviceSnapshot[]>("list_devices"),

  /** FR-9: a live Luci round trip, including the raw payloads for the debug pane. */
  getStatus: (uuid: string) => invoke<DeviceDetail>("get_status", { uuid }),

  /** Wake the device's poll loop instead of waiting out the interval. */
  refreshDevice: (uuid: string) => invoke<void>("refresh_device", { uuid }),

  // ------------------------------------------------------------ playback --

  /** Set absolute volume 0..100 over Luci `VOLUME(64)` (clamped in Rust). */
  setVolume: (uuid: string, vol: number) => invoke<void>("set_volume", { uuid, vol }),

  /** Mute/unmute over Luci `Mute_Unmute(63)`. */
  setMute: (uuid: string, mute: boolean) => invoke<void>("set_mute", { uuid, mute }),

  /** Transport: `play` | `pause` | `next` | `prev` | `stop` (UPnP, Luci fallback). */
  playerCmd: (uuid: string, cmd: PlayerCmd) => invoke<void>("player_cmd", { uuid, cmd }),

  /** Focus/blur → adaptive poll cadence (2 s focused / 5 s blurred). */
  setPollProfile: (focused: boolean) => invoke<void>("set_poll_profile", { focused }),

  /** This machine's LAN address — a hint for the manual-add form. */
  localAddress: () => invoke<string | null>("local_address"),

  // ---------------------------------------------------------- discovery --

  /**
   * FR-4: mDNS + SSDP + optional subnet sweep, run concurrently. Resolves with
   * the confirmed candidates; progress and hits stream as events meanwhile.
   * A second call cancels the running scan and restarts.
   */
  scan: (options: ScanOptions) => invoke<DeviceCandidate[]>("scan", { options }),

  /** Stop the running scan. Resolves `false` when nothing was running. */
  cancelScan: () => invoke<boolean>("cancel_scan"),

  // ----------------------------------------------------------- settings --

  getSettings: () => invoke<Settings>("get_settings"),

  /** FR-20 / FR-27: partial update (poll floor, theme, timeout, autostart). */
  updateSettings: (patch: SettingsPatch) => invoke<Settings>("update_settings", { patch }),

  /** FR-20: sweep default. `null` restores auto-detection. Validates the CIDR. */
  setSubnet: (cidr: string | null) => invoke<Settings>("set_subnet", { cidr }),

  /**
   * FR-21: pick a file and write settings + devices there (auth stripped).
   * Resolves `false` if the save dialog was cancelled.
   */
  exportConfigFile: () => invoke<boolean>("export_config_file"),

  /**
   * FR-21: pick a config file and merge it in (auth untouched). Resolves the
   * merged settings, or `null` if the open dialog was cancelled.
   */
  importConfigFile: () => invoke<Settings | null>("import_config_file"),

  // ------------------------------------------------------------ spotify (S3) --

  /**
   * Start advertising the "MusicSync" Spotify Connect endpoint. The user then
   * picks MusicSync in their Spotify app (Premium) to begin streaming.
   */
  spotifyStart: () => invoke<SpotifyState>("spotify_start"),

  /** Stop advertising and tear down the capture session. Idempotent. */
  spotifyStop: () => invoke<SpotifyState>("spotify_stop"),

  /** Current capture state (running / connected / now-playing). */
  spotifyStatus: () => invoke<SpotifyState>("spotify_status"),

  /** Transport, proxied to the Connect session. */
  spotifyPlay: () => invoke<void>("spotify_play"),
  spotifyPause: () => invoke<void>("spotify_pause"),
  spotifyNext: () => invoke<void>("spotify_next"),
  spotifyPrev: () => invoke<void>("spotify_prev"),

  /** Set Connect volume from a 0.0..=1.0 level. */
  spotifySetVolume: (level: number) => invoke<void>("spotify_set_volume", { level }),
};

/** True when the rejection is the Rust `{ code, message }` envelope. */
export function isAppError(e: unknown): e is AppErrorPayload {
  return typeof e === "object" && e !== null && "code" in e && "message" in e;
}

/** Best-effort human-readable message from any rejection. */
export function errorMessage(e: unknown, fallback = "Something went wrong."): string {
  if (isAppError(e)) return e.message;
  if (e instanceof Error) return e.message;
  if (typeof e === "string" && e.length > 0) return e;
  return fallback;
}

/** The stable machine code, e.g. `"locked_out"`. Empty for non-envelope errors. */
export function errorCode(e: unknown): string {
  return isAppError(e) ? e.code : "";
}
