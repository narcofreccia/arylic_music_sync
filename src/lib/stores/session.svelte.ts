import { commands } from "$lib/tauri/commands";

/**
 * Local-profile session (brief.md FR-1 / FR-2) — a reactive singleton fed by
 * the Rust `auth_state` command, which is the single source of truth (the Rust
 * side owns the Argon2 hash and the "remember me" grant).
 *
 * `init()` is idempotent: the root layout awaits it on mount and the route
 * guard reads the result, so concurrent navigations share one round-trip.
 */
class Session {
  /** True until the first `auth_state` resolves — the guard must not act yet. */
  loading = $state(true);
  configured = $state(false);
  loggedIn = $state(false);
  /** False once the password was removed (FR-3) — the app opens unprompted. */
  hasPassword = $state(false);
  username = $state<string | null>(null);
  rememberMe = $state(false);

  #started = false;
  #inflight: Promise<void> | null = null;

  /** Idempotent boot: safe to call from every route guard. */
  async init(): Promise<void> {
    if (this.#started) {
      // A second caller during boot still has to wait for the first answer.
      await this.#inflight;
      return;
    }
    this.#started = true;
    this.#inflight = this.refresh();
    await this.#inflight;
  }

  /** Re-read the authoritative state from Rust. */
  async refresh(): Promise<void> {
    try {
      const state = await commands.authState();
      this.configured = state.configured;
      this.loggedIn = state.configured && !state.requiresLogin;
      this.hasPassword = state.hasPassword;
      this.username = state.username;
      this.rememberMe = state.rememberMe;
    } catch (e) {
      // Core unreachable (or running outside Tauri): treat as unconfigured so
      // the user lands on the wizard rather than a dead screen.
      console.error("[session] auth_state failed:", e);
      this.configured = false;
      this.loggedIn = false;
    } finally {
      this.loading = false;
    }
  }

  /** FR-1. Throws the `{ code, message }` envelope on validation failure. */
  async createProfile(username: string, password: string, rememberMe: boolean): Promise<void> {
    await commands.createProfile(username, password);
    if (rememberMe) await commands.setRememberMe(true);
    await this.refresh();
  }

  /** FR-2. Returns false on wrong credentials; throws when throttled. */
  async login(username: string, password: string, rememberMe: boolean): Promise<boolean> {
    const ok = await commands.login(username, password);
    if (!ok) return false;
    // Only persist the grant once the credentials actually checked out.
    if (rememberMe) await commands.setRememberMe(true);
    await this.refresh();
    return true;
  }

  async logout(): Promise<void> {
    await commands.logout();
    await this.refresh();
  }

  /** FR-3, from Settings. */
  async setPassword(current: string, next: string): Promise<void> {
    await commands.setPassword(current, next);
  }

  /** FR-3: drop auth entirely — the app then opens without a login screen. */
  async removePassword(current: string): Promise<void> {
    await commands.removePassword(current);
    await this.refresh();
  }

  async setRememberMe(value: boolean): Promise<void> {
    await commands.setRememberMe(value);
    await this.refresh();
  }
}

export const session = new Session();
