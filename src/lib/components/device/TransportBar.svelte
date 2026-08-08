<script lang="ts">
  import { devices } from "$lib/stores/devices.svelte";
  import type { DeviceSnapshot, PlayerCmd } from "$lib/types";

  // Transport controls (R3). Shown only when a controllable source is active
  // (not idle) — an idle input has nothing to play/pause. Optimistic play/pause
  // is handled in the store; here we just render the current play-state.

  let { device }: { device: DeviceSnapshot } = $props();

  const playing = $derived(device.playState === 1);
  /** Idle (source 0) has no transport; unknown/active sources do. */
  const controllable = $derived(
    device.online && device.source !== null && device.source !== 0
  );

  function send(cmd: PlayerCmd) {
    void devices.player(device.uuid, cmd);
  }

  const btn =
    "flex size-8 items-center justify-center rounded-md border border-[var(--color-border-subtle)] text-slate-300 transition-colors hover:bg-[var(--color-surface)] disabled:opacity-40";
</script>

{#if controllable}
  <div class="flex items-center gap-1.5">
    <button type="button" class={btn} onclick={() => send("prev")} aria-label="Previous" title="Previous">
      <span aria-hidden="true">⏮</span>
    </button>

    <button
      type="button"
      class={btn}
      onclick={() => send(playing ? "pause" : "play")}
      aria-label={playing ? "Pause" : "Play"}
      title={playing ? "Pause" : "Play"}
    >
      <span aria-hidden="true">{playing ? "⏸" : "▶"}</span>
    </button>

    <button type="button" class={btn} onclick={() => send("next")} aria-label="Next" title="Next">
      <span aria-hidden="true">⏭</span>
    </button>

    <button type="button" class={btn} onclick={() => send("stop")} aria-label="Stop" title="Stop">
      <span aria-hidden="true">⏹</span>
    </button>
  </div>
{/if}
