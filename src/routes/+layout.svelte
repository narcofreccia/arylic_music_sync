<script lang="ts">
  import "../app.css";
  import favicon from "$lib/assets/favicon.svg";
  import { onMount } from "svelte";
  import { updates } from "$lib/stores/updates.svelte";

  let { children } = $props();

  onMount(() => {
    // Silent launch check — offline/LAN-only use is normal, so never surfaces.
    void updates.start();
  });
</script>

<svelte:head><link rel="icon" href={favicon} /></svelte:head>

<div class="flex min-h-screen flex-col bg-[var(--color-surface)] text-slate-200">
  <!-- macOS uses an overlay title bar, so the header doubles as the drag region. -->
  <header
    data-tauri-drag-region
    class="flex h-12 shrink-0 items-center gap-3 border-b border-[var(--color-border-subtle)] px-4 pl-20 select-none"
  >
    <span class="text-sm font-semibold tracking-wide">MusicSync</span>
    <span class="text-xs text-slate-500">LP10 Multiroom</span>
  </header>

  <main class="flex-1 overflow-auto p-6">
    {@render children()}
  </main>
</div>
