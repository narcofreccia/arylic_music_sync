import { commands } from "$lib/tauri/commands";
import type { Theme } from "$lib/types";

/**
 * UI theme (FR-27). The chosen mode (`dark` | `light` | `system`) is persisted
 * in settings.json by Rust; this store mirrors it, resolves `system` against the
 * OS preference, and stamps `data-theme` on `<html>` so the CSS token overrides
 * in app.css take hold. Dark is the default and the app's native look.
 */
const THEMES: Theme[] = ["dark", "light", "system"];

function normalize(value: string): Theme {
  return (THEMES as string[]).includes(value) ? (value as Theme) : "dark";
}

class ThemeStore {
  /** The user's choice, including `system`. */
  mode = $state<Theme>("dark");
  /** The concrete look currently applied. */
  resolved = $state<"dark" | "light">("dark");

  #media: MediaQueryList | null = null;
  #onMedia = () => {
    if (this.mode === "system") this.#apply();
  };

  /** Boot from the persisted value. Idempotent — safe to call on every mount. */
  init(mode: string): void {
    this.mode = normalize(mode);
    if (typeof window !== "undefined" && !this.#media) {
      this.#media = window.matchMedia("(prefers-color-scheme: light)");
      this.#media.addEventListener("change", this.#onMedia);
    }
    this.#apply();
  }

  /** Change the theme, apply it immediately, and persist through Rust. */
  async set(mode: Theme): Promise<void> {
    this.mode = mode;
    this.#apply();
    await commands.updateSettings({ theme: mode });
  }

  #apply(): void {
    const light = this.mode === "light" || (this.mode === "system" && !!this.#media?.matches);
    this.resolved = light ? "light" : "dark";
    if (typeof document !== "undefined") {
      document.documentElement.setAttribute("data-theme", this.resolved);
    }
  }
}

export const theme = new ThemeStore();
