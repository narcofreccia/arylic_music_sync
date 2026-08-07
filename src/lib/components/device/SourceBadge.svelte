<script lang="ts">
  import type { SourceMode } from "$lib/types";

  // Active input per device (FR-19). Unknown firmware codes arrive as
  // "mode <n>" from Rust and are shown as-is rather than hidden.

  let { source }: { source: SourceMode } = $props();

  const LABELS: Record<string, string> = {
    idle: "Idle",
    airplay: "AirPlay",
    dlna: "DLNA",
    network: "Wi-Fi stream",
    usb: "USB",
    spotify: "Spotify Connect",
    "line-in": "Line-in",
    bluetooth: "Bluetooth",
    optical: "Optical",
    "line-in2": "Line-in 2",
    "usb-dac": "USB DAC",
    follower: "Following master"
  };

  const label = $derived(LABELS[source] ?? source);
</script>

<span
  class="rounded-full border border-[var(--color-border-subtle)] px-2 py-0.5 text-[11px] text-slate-400"
>
  {label}
</span>
