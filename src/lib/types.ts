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

/** Persisted device entry — defined in M1, populated by M2's Linkplay client. */
export interface SavedDevice {
  id: string;
  name: string;
  ip: string;
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
