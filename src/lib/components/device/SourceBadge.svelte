<script lang="ts">
  // Active input per device, from Luci `CURRSOURCE`. The integer→name mapping for
  // the LP10 is not yet field-verified, so a known code gets a label and anything
  // else is shown as its raw number rather than guessed at.

  // Prefer the Rust-side label (`device.sourceLabel`), which centralises the
  // field-verified mapping (docs/firmware-notes.md); fall back to the raw code.
  let { source, label: given = null }: { source: number | null; label?: string | null } =
    $props();

  const label = $derived(given ?? (source === null ? "" : `Source ${source}`));
</script>

{#if label}
  <span
    class="rounded-full border border-[var(--color-border-subtle)] px-2 py-0.5 text-[11px] text-slate-400"
  >
    {label}
  </span>
{/if}
