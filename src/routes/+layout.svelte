<script lang="ts">
  import "../app.css";
  import favicon from "$lib/assets/favicon.svg";
  import { onMount } from "svelte";
  import { goto } from "$app/navigation";
  import { page } from "$app/state";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import type { UnlistenFn } from "@tauri-apps/api/event";
  import { updates } from "$lib/stores/updates.svelte";
  import { session } from "$lib/stores/session.svelte";
  import { devices } from "$lib/stores/devices.svelte";
  import { scan } from "$lib/stores/scan.svelte";
  import { toasts } from "$lib/stores/toasts.svelte";
  import { commands } from "$lib/tauri/commands";

  let { children } = $props();

  /** Auth screens own the whole window — no header, no nav (FR-1/FR-2). */
  const AUTH_ROUTES = ["/setup", "/login"];

  const path = $derived(page.url.pathname);
  const chromeless = $derived(AUTH_ROUTES.includes(path));

  onMount(() => {
    void session.init();
    // Silent launch check — offline/LAN-only use is normal, so never surfaces.
    void updates.start();

    // Adaptive polling: tell Rust when the window gains/loses focus so the
    // poller can slow down (5 s) while the user isn't looking, speed up (2 s)
    // when they are.
    let unlistenFocus: UnlistenFn | undefined;
    void getCurrentWindow()
      .onFocusChanged(({ payload: focused }) => void commands.setPollProfile(focused))
      .then((off) => (unlistenFocus = off))
      .catch((e) => console.error("[layout] focus listener failed:", e));

    return () => {
      devices.stop();
      scan.stop();
      unlistenFocus?.();
    };
  });

  // Device and scan events are subscribed here, once, and only after auth
  // clears — a page-level listener would stack a new subscription per
  // navigation. Both start() calls are idempotent, so re-running this effect is
  // harmless. The scan store must be listening before the devices page can
  // start a scan, hence here rather than on that page.
  $effect(() => {
    if (session.loggedIn) {
      void devices.start();
      void scan.start();
    }
  });

  // Route guard. Runs on every session/route change; each branch only navigates
  // when already off-target, so it settles instead of looping.
  $effect(() => {
    if (session.loading) return;
    if (!session.configured) {
      if (path !== "/setup") void goto("/setup");
    } else if (!session.loggedIn) {
      if (path !== "/login") void goto("/login");
    } else if (AUTH_ROUTES.includes(path)) {
      void goto("/");
    }
  });

  // Don't paint the app shell behind a pending redirect — it would flash the
  // dashboard at a user who still has to log in.
  const blocked = $derived(
    session.loading ||
      (!session.configured && path !== "/setup") ||
      (session.configured && !session.loggedIn && path !== "/login") ||
      (session.configured && session.loggedIn && chromeless)
  );
</script>

<svelte:head><link rel="icon" href={favicon} /></svelte:head>

{#if blocked}
  <!-- Drag region kept so the frameless window stays movable while booting. -->
  <div
    data-tauri-drag-region
    class="flex min-h-screen items-center justify-center bg-[var(--color-surface)] text-sm text-slate-500"
  >
    Loading…
  </div>
{:else if chromeless}
  <div
    data-tauri-drag-region
    class="flex min-h-screen items-center justify-center bg-[var(--color-surface)] p-6 text-slate-200"
  >
    {@render children()}
  </div>
{:else}
  <div class="flex min-h-screen flex-col bg-[var(--color-surface)] text-slate-200">
    <!-- macOS uses an overlay title bar, so the header doubles as the drag region. -->
    <header
      data-tauri-drag-region
      class="flex h-12 shrink-0 items-center gap-3 border-b border-[var(--color-border-subtle)] px-4 pl-20 select-none"
    >
      <span class="text-sm font-semibold tracking-wide">MusicSync</span>
      <span class="text-xs text-slate-500">LP10 Multiroom</span>

      <nav class="ml-auto flex items-center gap-1 text-xs">
        <a
          href="/"
          class="rounded px-2 py-1 transition-colors hover:bg-[var(--color-surface-raised)] {path ===
          '/'
            ? 'text-white'
            : 'text-slate-400'}">Dashboard</a
        >
        <a
          href="/devices"
          class="rounded px-2 py-1 transition-colors hover:bg-[var(--color-surface-raised)] {path.startsWith(
            '/devices'
          )
            ? 'text-white'
            : 'text-slate-400'}">Devices</a
        >
        <a
          href="/settings"
          class="rounded px-2 py-1 transition-colors hover:bg-[var(--color-surface-raised)] {path ===
          '/settings'
            ? 'text-white'
            : 'text-slate-400'}">Settings</a
        >
      </nav>
    </header>

    <main class="flex-1 overflow-auto p-6">
      {@render children()}
    </main>
  </div>
{/if}

<!-- Command-failure toasts (R3, NFR-2): rollback notifications from the store. -->
{#if toasts.items.length > 0}
  <div class="pointer-events-none fixed inset-x-0 bottom-4 z-50 flex flex-col items-center gap-2 px-4">
    {#each toasts.items as toast (toast.id)}
      <div
        role="status"
        class="pointer-events-auto flex max-w-md items-start gap-3 rounded-lg border px-4 py-2.5 text-sm shadow-lg {toast.tone ===
        'error'
          ? 'border-red-500/40 bg-red-500/15 text-red-200'
          : 'border-[var(--color-border-subtle)] bg-[var(--color-surface-raised)] text-slate-200'}"
      >
        <span class="flex-1">{toast.message}</span>
        <button
          type="button"
          onclick={() => toasts.dismiss(toast.id)}
          class="shrink-0 text-slate-400 transition-colors hover:text-white"
          aria-label="Dismiss"
        >
          ✕
        </button>
      </div>
    {/each}
  </div>
{/if}
