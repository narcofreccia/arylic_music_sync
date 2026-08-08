<script lang="ts">
  import { devices } from "$lib/stores/devices.svelte";
  import { errorMessage } from "$lib/tauri/commands";
  import { sinceLabel } from "$lib/format";
  import type { DeviceDetail, DeviceSnapshot } from "$lib/types";

  // Device detail (FR-9). The identity/firmware fields come from a *live*
  // `get_status` round trip rather than the cached snapshot — this pane is also
  // the tool for the FR-23 firmware spike, so it must show what the device says
  // right now, including every field we don't model yet.

  let { device }: { device: DeviceSnapshot } = $props();

  let detail = $state<DeviceDetail | null>(null);
  let loading = $state(true);
  let error = $state("");
  let showRaw = $state(false);

  async function load(uuid: string) {
    loading = true;
    error = "";
    try {
      detail = await devices.detail(uuid);
    } catch (e) {
      detail = null;
      error = errorMessage(e, "Could not reach the device.");
    } finally {
      loading = false;
    }
  }

  $effect(() => {
    void load(device.uuid);
  });

  // Fall back to the cached snapshot so the pane still shows identity fields
  // when the device is offline.
  const shown = $derived(detail?.snapshot ?? device);

  const netLabel = $derived(
    shown.netMode === "ethernet"
      ? "Wired (Ethernet)"
      : shown.netMode === "wifi"
        ? `Wi-Fi${shown.wifiBand ? ` (${shown.wifiBand})` : ""}`
        : "—"
  );

  const rows = $derived([
    ["IP address", shown.ip],
    ["UUID", shown.uuid],
    ["Model", shown.model || "—"],
    ["Firmware", shown.firmware || "—"],
    ["Connection", netLabel],
    ["Role", shown.role.charAt(0).toUpperCase() + shown.role.slice(1)],
    ["Group id", shown.groupId ?? "—"],
    ["Volume", shown.volume === null ? "—" : `${shown.volume}${shown.mute ? " (muted)" : ""}`],
    ["Source", shown.source === null ? "—" : `${shown.source}`],
    ["Play state", shown.playState === null ? "—" : shown.playState === 1 ? "Playing" : "Stopped"],
    ["Last seen", shown.online ? "now" : sinceLabel(shown.lastSeen)]
  ] as const);

  const track = $derived(shown.track);

  const raw = $derived(detail ? JSON.stringify(detail.raw, null, 2) : "");
</script>

<div class="border-t border-[var(--color-border-subtle)] px-4 py-4 text-sm">
  {#if loading}
    <p class="text-slate-500">Reading device…</p>
  {:else if error}
    <p class="rounded-md bg-red-500/10 px-3 py-2 text-red-300">{error}</p>
  {/if}

  <dl class="mt-1 grid grid-cols-[9rem_1fr] gap-x-4 gap-y-2">
    {#each rows as [label, value] (label)}
      <dt class="text-slate-500">{label}</dt>
      <dd class="font-mono text-xs break-all text-slate-300">{value}</dd>
    {/each}
  </dl>

  {#if track && (track.title || track.artist || track.album)}
    <h4 class="mt-4 text-xs font-medium tracking-wide text-slate-400 uppercase">Now playing</h4>
    <p class="mt-1 text-xs text-slate-300">
      {[track.title, track.artist, track.album].filter(Boolean).join(" · ")}
    </p>
  {/if}

  {#if detail}
    <!-- Raw Luci/DDMS payloads, verbatim: the input for docs/firmware-notes.md. -->
    <button
      type="button"
      onclick={() => (showRaw = !showRaw)}
      class="mt-4 text-xs text-slate-400 underline underline-offset-2 hover:text-slate-200"
    >
      {showRaw ? "Hide" : "Show"} raw response fields
    </button>
    {#if showRaw}
      <pre
        class="mt-2 max-h-72 overflow-auto rounded-md border border-[var(--color-border-subtle)] bg-[var(--color-surface)] p-3 text-[11px] leading-relaxed text-slate-400">{raw}</pre>
    {/if}
  {/if}
</div>
