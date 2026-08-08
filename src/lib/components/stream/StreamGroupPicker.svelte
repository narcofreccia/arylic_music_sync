<script lang="ts">
  import { devices } from "$lib/stores/devices.svelte";
  import NetBadge from "$lib/components/device/NetBadge.svelte";

  // Speaker group picker: the discovered devices as checkboxes that build the
  // stream group. `selected` is a bindable list of device UUIDs owned by the
  // page. Offline devices can't receive a stream, so they're shown disabled.

  let { selected = $bindable([]), disabled = false }: { selected: string[]; disabled?: boolean } =
    $props();

  function toggle(uuid: string) {
    selected = selected.includes(uuid)
      ? selected.filter((u) => u !== uuid)
      : [...selected, uuid];
  }
</script>

{#if devices.loading}
  <p class="text-sm text-slate-500">Loading speakers…</p>
{:else if devices.count === 0}
  <div
    class="rounded-lg border border-dashed border-[var(--color-border-subtle)] p-6 text-sm text-slate-500"
  >
    No speakers yet. <a
      href="/devices"
      class="text-[var(--color-accent)] underline underline-offset-2">Scan for speakers</a
    > first, then come back to play everywhere.
  </div>
{:else}
  <ul class="flex flex-col gap-2">
    {#each devices.list as device (device.uuid)}
      {@const checked = selected.includes(device.uuid)}
      <li>
        <label
          class="flex items-center gap-3 rounded-md border px-3 py-2 transition-colors {device.online &&
          !disabled
            ? 'cursor-pointer border-[var(--color-border-subtle)] bg-[var(--color-surface)] hover:bg-[var(--color-surface-raised)]'
            : 'cursor-not-allowed border-[var(--color-border-subtle)] bg-[var(--color-surface)] opacity-50'}"
        >
          <input
            type="checkbox"
            {checked}
            disabled={disabled || !device.online}
            onchange={() => toggle(device.uuid)}
            class="size-4 shrink-0 accent-[var(--color-accent)]"
          />
          <span class="min-w-0 flex-1 truncate text-sm text-slate-200">{device.displayName}</span>
          <span class="font-mono text-xs text-slate-500">{device.ip}</span>
          {#if device.online}
            <NetBadge netMode={device.netMode} wifiBand={device.wifiBand} />
          {:else}
            <span class="text-xs text-slate-500">offline</span>
          {/if}
        </label>
      </li>
    {/each}
  </ul>
{/if}
