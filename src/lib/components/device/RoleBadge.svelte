<script lang="ts">
  import type { DeviceRole } from "$lib/types";

  // Group role at a glance (FR-9/FR-13). The master badge is deliberately the
  // loud one: it is the entry the user must pick in Spotify Connect (FR-26).

  let { role }: { role: DeviceRole } = $props();

  const label = $derived(
    role.kind === "master"
      ? `Master · ${role.slaveUuids.length} slave${role.slaveUuids.length === 1 ? "" : "s"}`
      : role.kind === "slave"
        ? "Slave"
        : "Solo"
  );

  const tone = $derived(
    role.kind === "master"
      ? "border-[var(--color-accent)]/50 bg-[var(--color-accent)]/10 text-[var(--color-accent)]"
      : role.kind === "slave"
        ? "border-violet-400/40 bg-violet-400/10 text-violet-300"
        : "border-[var(--color-border-subtle)] text-slate-400"
  );
</script>

<span class="rounded-full border px-2 py-0.5 text-[11px] font-medium {tone}">{label}</span>
