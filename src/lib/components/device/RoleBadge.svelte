<script lang="ts">
  import type { Role } from "$lib/types";

  // Group role at a glance, read from the DDMS `State` banner. R1 only shows it;
  // grouping actions arrive in R2.

  let { role }: { role: Role } = $props();

  const label = $derived(role === "master" ? "Master" : role === "slave" ? "Slave" : "Solo");

  const tone = $derived(
    role === "master"
      ? "border-[var(--color-accent)]/50 bg-[var(--color-accent)]/10 text-[var(--color-accent)]"
      : role === "slave"
        ? "border-violet-400/40 bg-violet-400/10 text-violet-300"
        : "border-[var(--color-border-subtle)] text-slate-400"
  );
</script>

{#if role !== "solo"}
  <span class="rounded-full border px-2 py-0.5 text-[11px] font-medium {tone}">{label}</span>
{/if}
