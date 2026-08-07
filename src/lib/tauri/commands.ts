import { invoke } from "@tauri-apps/api/core";
import type { AppErrorPayload, AuthState } from "$lib/types";

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
