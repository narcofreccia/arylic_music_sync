<script lang="ts">
  // Active input per device, from Luci `CURRSOURCE`. The integer→name mapping for
  // the LP10 is not yet field-verified, so a known code gets a label and anything
  // else is shown as its raw number rather than guessed at.

  let { source }: { source: number | null } = $props();

  const LABELS: Record<number, string> = {
    0: "Idle",
    1: "AirPlay",
    2: "DLNA",
    3: "Wi-Fi stream",
    4: "Bluetooth",
    5: "Line-in",
    6: "Optical",
    10: "Network"
  };

  const label = $derived(source === null ? "" : (LABELS[source] ?? `Source ${source}`));
</script>

{#if label}
  <span
    class="rounded-full border border-[var(--color-border-subtle)] px-2 py-0.5 text-[11px] text-slate-400"
  >
    {label}
  </span>
{/if}
