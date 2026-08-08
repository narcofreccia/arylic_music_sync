<script lang="ts">
  import { devices } from "$lib/stores/devices.svelte";
  import { errorMessage } from "$lib/tauri/commands";
  import { clock, sinceLabel } from "$lib/format";
  import DeviceDetailPane from "./DeviceDetail.svelte";
  import RoleBadge from "./RoleBadge.svelte";
  import SourceBadge from "./SourceBadge.svelte";
  import NetBadge from "./NetBadge.svelte";
  import type { DeviceSnapshot } from "$lib/types";

  // One row of the device list (FR-6 … FR-9). Actions go straight to the
  // devices store: it is a singleton fed by the Rust poller, so there is no
  // local copy of device state to keep in sync here.

  let { device }: { device: DeviceSnapshot } = $props();

  let expanded = $state(false);
  let renaming = $state(false);
  let confirmingRemove = $state(false);
  let busy = $state(false);
  let error = $state("");

  let aliasDraft = $state("");
  let pushToDevice = $state(false);

  const nowPlaying = $derived.by(() => {
    if (device.playState !== 1) return "";
    const track = device.track;
    const label = [track?.title, track?.artist].filter(Boolean).join(" — ");
    return label || "Playing";
  });

  function startRename() {
    aliasDraft = device.alias ?? device.name;
    pushToDevice = false;
    error = "";
    renaming = true;
  }

  async function saveRename(event: SubmitEvent) {
    event.preventDefault();
    busy = true;
    error = "";
    try {
      // An emptied field clears the alias and falls back to the device's name.
      const alias = aliasDraft.trim() === "" ? null : aliasDraft.trim();
      await devices.rename(device.uuid, alias, pushToDevice);
      renaming = false;
    } catch (e) {
      error = errorMessage(e, "Could not rename the device.");
    } finally {
      busy = false;
    }
  }

  async function remove() {
    busy = true;
    error = "";
    try {
      await devices.remove(device.uuid);
    } catch (e) {
      error = errorMessage(e, "Could not remove the device.");
      busy = false;
      confirmingRemove = false;
    }
  }

  async function refresh() {
    busy = true;
    try {
      await devices.refresh(device.uuid);
    } catch (e) {
      error = errorMessage(e, "Could not refresh the device.");
    } finally {
      busy = false;
    }
  }

  const action =
    "rounded-md border border-[var(--color-border-subtle)] px-2.5 py-1 text-xs text-slate-300 transition-colors hover:bg-[var(--color-surface)] disabled:opacity-50";
</script>

<li
  class="rounded-lg border border-[var(--color-border-subtle)] bg-[var(--color-surface-raised)] {device.online
    ? ''
    : 'opacity-70'}"
>
  <div class="flex flex-wrap items-start gap-x-4 gap-y-2 p-4">
    <div class="min-w-0 flex-1">
      <div class="flex flex-wrap items-center gap-2">
        <span
          class="size-2 shrink-0 rounded-full {device.online ? 'bg-emerald-400' : 'bg-slate-600'}"
          aria-hidden="true"
        ></span>
        <h3 class="truncate text-sm font-medium text-white">{device.displayName}</h3>
        <span class="text-xs text-slate-500">{device.ip}</span>
        {#if device.online}
          <NetBadge netMode={device.netMode} wifiBand={device.wifiBand} />
        {/if}
        <RoleBadge role={device.role} />
        {#if device.online && device.source !== null}
          <SourceBadge source={device.source} />
        {/if}
      </div>

      <p class="mt-1 truncate text-xs text-slate-500">
        {#if !device.online}
          Offline · {sinceLabel(device.lastSeen)}
        {:else if nowPlaying}
          ♪ {nowPlaying}
          {#if device.track && device.track.durationMs}
            <span class="text-slate-600">
              · {clock(device.track.positionMs ?? 0)} / {clock(device.track.durationMs)}</span
            >
          {/if}
        {:else if device.role === "slave"}
          Following the group master
        {:else if device.volume !== null}
          Volume {device.volume}{device.mute ? " · muted" : ""}
          {#if device.model}· {device.model}{/if}
        {:else}
          Online
        {/if}
      </p>
    </div>

    <div class="flex shrink-0 flex-wrap items-center gap-1.5">
      <button type="button" class={action} disabled={busy} onclick={refresh}>Refresh</button>
      <button type="button" class={action} disabled={busy} onclick={startRename}>Rename</button>
      <button
        type="button"
        class={action}
        aria-expanded={expanded}
        onclick={() => (expanded = !expanded)}>{expanded ? "Hide" : "Details"}</button
      >
      <button
        type="button"
        class="rounded-md border border-red-500/40 px-2.5 py-1 text-xs text-red-300 transition-colors hover:bg-red-500/10 disabled:opacity-50"
        disabled={busy}
        onclick={() => (confirmingRemove = true)}>Remove</button
      >
    </div>
  </div>

  {#if error}
    <p class="mx-4 mb-3 rounded-md bg-red-500/10 px-3 py-2 text-xs text-red-300">{error}</p>
  {/if}

  {#if renaming}
    <!-- FR-7: local alias, with the optional push to the device itself. -->
    <form onsubmit={saveRename} class="flex flex-col gap-3 border-t border-[var(--color-border-subtle)] px-4 py-3">
      <label class="flex flex-col gap-1 text-xs text-slate-400">
        Friendly name
        <input
          bind:value={aliasDraft}
          placeholder={device.name || device.ip}
          class="rounded-md border border-[var(--color-border-subtle)] bg-[var(--color-surface)] px-3 py-2 text-sm text-white outline-none focus:border-[var(--color-accent)]"
        />
      </label>
      <label class="flex items-center gap-2 text-xs text-slate-400">
        <input type="checkbox" bind:checked={pushToDevice} class="size-4 accent-[var(--color-accent)]" />
        Also rename the device itself (changes the name shown in Spotify Connect)
      </label>
      <div class="flex gap-2">
        <button
          type="submit"
          disabled={busy}
          class="rounded-md bg-[var(--color-accent)] px-3 py-1.5 text-xs font-medium text-slate-900 transition-opacity hover:opacity-90 disabled:opacity-50"
        >
          {busy ? "Saving…" : "Save"}
        </button>
        <button type="button" class={action} onclick={() => (renaming = false)}>Cancel</button>
      </div>
    </form>
  {/if}

  {#if confirmingRemove}
    <!-- Inline confirm, matching the Settings page: no native dialogs. -->
    <div class="flex flex-wrap items-center gap-3 border-t border-[var(--color-border-subtle)] px-4 py-3">
      <p class="text-xs text-slate-300">
        Remove <span class="text-white">{device.displayName}</span> from the list? The device
        itself is not changed.
      </p>
      <div class="flex gap-2">
        <button
          type="button"
          disabled={busy}
          onclick={remove}
          class="rounded-md bg-red-500/80 px-3 py-1.5 text-xs font-medium text-white transition-opacity hover:opacity-90 disabled:opacity-50"
        >
          {busy ? "Removing…" : "Yes, remove"}
        </button>
        <button type="button" class={action} onclick={() => (confirmingRemove = false)}>Cancel</button>
      </div>
    </div>
  {/if}

  {#if expanded}
    <DeviceDetailPane {device} />
  {/if}
</li>
