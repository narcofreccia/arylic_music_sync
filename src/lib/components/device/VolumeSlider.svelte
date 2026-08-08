<script lang="ts">
  import { devices } from "$lib/stores/devices.svelte";
  import type { DeviceSnapshot } from "$lib/types";

  // Per-device volume + mute (R3, NFR-2). The store owns the optimistic override
  // and its rollback, so this component just reflects `device.volume`/`.mute`
  // (already merged with any in-flight change) and fires intents at the store.

  let { device }: { device: DeviceSnapshot } = $props();

  const disabled = $derived(!device.online || device.volume === null);
  const value = $derived(device.volume ?? 0);

  function onInput(event: Event) {
    const next = Number((event.currentTarget as HTMLInputElement).value);
    devices.setVolume(device.uuid, next);
  }

  function toggleMute() {
    if (!device.online) return;
    void devices.setMute(device.uuid, !device.mute);
  }
</script>

<div class="flex items-center gap-2">
  <button
    type="button"
    onclick={toggleMute}
    {disabled}
    aria-pressed={device.mute}
    aria-label={device.mute ? "Unmute" : "Mute"}
    title={device.mute ? "Unmute" : "Mute"}
    class="shrink-0 rounded-md border border-[var(--color-border-subtle)] px-1.5 py-0.5 text-xs transition-colors hover:bg-[var(--color-surface)] disabled:opacity-40 {device.mute
      ? 'text-amber-300'
      : 'text-slate-400'}"
  >
    <span aria-hidden="true">{device.mute ? "\u{1F507}" : "\u{1F509}"}</span>
  </button>

  <input
    type="range"
    min="0"
    max="100"
    step="1"
    {value}
    {disabled}
    oninput={onInput}
    aria-label="Volume"
    class="h-1.5 w-full cursor-pointer appearance-none rounded-full bg-[var(--color-surface)] accent-[var(--color-accent)] disabled:cursor-default disabled:opacity-40"
  />

  <span class="w-7 shrink-0 text-right font-mono text-xs text-slate-400 tabular-nums">
    {disabled ? "—" : value}
  </span>
</div>
