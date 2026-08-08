<script lang="ts">
  import type { NetMode } from "$lib/types";

  // Wired vs Wi-Fi badge (R1's headline distinction), with the Wi-Fi band when
  // known. The wired unit is the natural group master, so the badge is load-bearing.

  let { netMode, wifiBand }: { netMode: NetMode | null; wifiBand: string | null } = $props();

  const label = $derived(
    netMode === "ethernet"
      ? "Wired"
      : netMode === "wifi"
        ? wifiBand && wifiBand !== "ETH"
          ? `Wi-Fi ${wifiBand}`
          : "Wi-Fi"
        : ""
  );

  const tone = $derived(
    netMode === "ethernet"
      ? "border-sky-400/40 bg-sky-400/10 text-sky-300"
      : "border-amber-400/40 bg-amber-400/10 text-amber-300"
  );
</script>

{#if label}
  <span
    class="inline-flex items-center gap-1 rounded-full border px-2 py-0.5 text-[11px] font-medium {tone}"
  >
    <span aria-hidden="true">{netMode === "ethernet" ? "\u{1F50C}" : "\u{1F4F6}"}</span>
    {label}
  </span>
{/if}
