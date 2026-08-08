<script lang="ts">
  import { stream } from "$lib/stores/stream.svelte";
  import { devices } from "$lib/stores/devices.svelte";
  import NetBadge from "$lib/components/device/NetBadge.svelte";
  import type { StreamDeviceStatus } from "$lib/types";

  // One selected speaker in a live stream: name + wired/Wi-Fi badge + sender
  // health, plus a volume slider (0..100 → 0..1) and a delay (ms) slider. Both
  // are optimistic in the store, so this component just reflects and fires.

  let { room }: { room: StreamDeviceStatus } = $props();

  /** Max per-room delay the slider offers (ms) — enough to trim room-to-room skew. */
  const MAX_DELAY_MS = 500;

  // Match the live sender IP back to a discovered device for its net badge.
  const device = $derived(devices.list.find((d) => d.ip === room.ip));

  const volumePct = $derived(Math.round(room.volume * 100));

  function onVolume(event: Event) {
    const pct = Number((event.currentTarget as HTMLInputElement).value);
    stream.setDeviceVolume(room.ip, pct / 100);
  }

  function onDelay(event: Event) {
    const ms = Number((event.currentTarget as HTMLInputElement).value);
    stream.setDeviceDelay(room.ip, ms);
  }
</script>

<div class="rounded-md border border-[var(--color-border-subtle)] bg-[var(--color-surface)] p-3">
  <div class="flex items-center gap-2">
    <span
      class="size-2 shrink-0 rounded-full {room.alive ? 'bg-emerald-400' : 'bg-red-400'}"
      title={room.alive ? "Sender running" : "Sender stopped"}
      aria-hidden="true"
    ></span>
    <span class="truncate text-sm font-medium text-white">{room.name}</span>
    <span class="font-mono text-xs text-slate-500">{room.ip}</span>
    {#if device}
      <NetBadge netMode={device.netMode} wifiBand={device.wifiBand} />
    {/if}
    {#if !room.alive}
      <span class="ml-auto text-xs text-red-300">disconnected</span>
    {/if}
  </div>

  <div class="mt-3 flex items-center gap-3">
    <label class="w-14 shrink-0 text-xs text-slate-400" for="vol-{room.ip}">Volume</label>
    <input
      id="vol-{room.ip}"
      type="range"
      min="0"
      max="100"
      step="1"
      value={volumePct}
      oninput={onVolume}
      aria-label="{room.name} volume"
      class="h-1.5 w-full cursor-pointer appearance-none rounded-full bg-[var(--color-surface-raised)] accent-[var(--color-accent)]"
    />
    <span class="w-8 shrink-0 text-right font-mono text-xs text-slate-400 tabular-nums"
      >{volumePct}</span
    >
  </div>

  <div class="mt-2 flex items-center gap-3">
    <label class="w-14 shrink-0 text-xs text-slate-400" for="delay-{room.ip}">Delay</label>
    <input
      id="delay-{room.ip}"
      type="range"
      min="0"
      max={MAX_DELAY_MS}
      step="5"
      value={room.delay_ms}
      oninput={onDelay}
      aria-label="{room.name} delay in milliseconds"
      class="h-1.5 w-full cursor-pointer appearance-none rounded-full bg-[var(--color-surface-raised)] accent-[var(--color-accent)]"
    />
    <span class="w-12 shrink-0 text-right font-mono text-xs text-slate-400 tabular-nums"
      >{room.delay_ms}ms</span
    >
  </div>
</div>
