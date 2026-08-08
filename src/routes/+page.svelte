<script lang="ts">
  import RoleBadge from "$lib/components/device/RoleBadge.svelte";
  import SourceBadge from "$lib/components/device/SourceBadge.svelte";
  import NetBadge from "$lib/components/device/NetBadge.svelte";
  import VolumeSlider from "$lib/components/device/VolumeSlider.svelte";
  import NowPlaying from "$lib/components/device/NowPlaying.svelte";
  import TransportBar from "$lib/components/device/TransportBar.svelte";
  import { devices } from "$lib/stores/devices.svelte";
  import { sinceLabel } from "$lib/format";

  // Live dashboard, fed by the poller through the devices store. Per-device
  // cards carry volume, mute, transport and now-playing (R3); optimistic control
  // with rollback lives in the store.
</script>

<svelte:head><title>MusicSync</title></svelte:head>

<div class="mx-auto flex max-w-4xl flex-col gap-6">
  <div>
    <h1 class="text-2xl font-semibold text-white">MusicSync</h1>
    <p class="mt-1 text-sm text-slate-400">
      Discover, group and control Arylic LP10 streamers on your local network.
    </p>
  </div>

  {#if devices.loading}
    <p class="text-sm text-slate-500">Loading devices…</p>
  {:else if devices.count === 0}
    <div
      class="rounded-lg border border-[var(--color-border-subtle)] bg-[var(--color-surface-raised)] p-6"
    >
      <h2 class="text-sm font-medium text-slate-300">No devices yet</h2>
      <p class="mt-2 text-sm text-slate-500">
        Add your LP10s by IP on the <a
          href="/devices"
          class="text-[var(--color-accent)] underline underline-offset-2">Devices</a
        > page. Grouping and playback controls arrive in the next milestones.
      </p>
    </div>
  {:else}
    <div class="flex items-baseline justify-between">
      <p class="text-sm text-slate-400">
        {devices.online.length} of {devices.count} online{#if devices.grouped.length > 0}
          · {devices.grouped.length} grouped{/if}
      </p>
      <a href="/devices" class="text-xs text-[var(--color-accent)] underline underline-offset-2">
        Manage devices
      </a>
    </div>

    <ul class="grid gap-3 sm:grid-cols-2">
      {#each devices.list as device (device.uuid)}
        <li
          class="rounded-lg border border-[var(--color-border-subtle)] bg-[var(--color-surface-raised)] p-4 {device.online
            ? ''
            : 'opacity-70'}"
        >
          <div class="flex items-center gap-2">
            <span
              class="size-2 shrink-0 rounded-full {device.online ? 'bg-emerald-400' : 'bg-slate-600'}"
              aria-hidden="true"
            ></span>
            <h2 class="truncate text-sm font-medium text-white">{device.displayName}</h2>
          </div>

          <div class="mt-2 flex flex-wrap items-center gap-2">
            {#if device.online}
              <NetBadge netMode={device.netMode} wifiBand={device.wifiBand} />
            {/if}
            <RoleBadge role={device.role} />
            {#if device.online && device.source !== null}
              <SourceBadge source={device.source} label={device.sourceLabel} />
            {/if}
          </div>

          {#if !device.online}
            <p class="mt-2 truncate text-xs text-slate-500">
              Offline · {sinceLabel(device.lastSeen)}
            </p>
          {:else if device.role === "slave"}
            <p class="mt-2 truncate text-xs text-slate-500">Following the group master</p>
          {:else}
            <div class="mt-3 flex flex-col gap-2.5">
              {#if device.volume !== null}
                <VolumeSlider {device} />
              {/if}
              <NowPlaying {device} />
              <TransportBar {device} />
            </div>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
</div>
