<script lang="ts">
  import { onMount } from "svelte";
  import DeviceCard from "$lib/components/device/DeviceCard.svelte";
  import { devices } from "$lib/stores/devices.svelte";
  import { commands, errorMessage } from "$lib/tauri/commands";

  // Devices page (FR-5 … FR-9). Manual add only for now — the mDNS/SSDP scan
  // and the subnet sweep (FR-4) arrive with M3.
  //
  // Event listeners are registered once by the root layout via devices.start();
  // this page only reads the store, so navigating away and back doesn't stack
  // subscriptions.

  let ip = $state("");
  let adding = $state(false);
  let addError = $state("");
  let addedName = $state("");

  /** This machine's LAN address, purely as a hint for what to type. */
  let localIp = $state<string | null>(null);

  onMount(async () => {
    try {
      localIp = await commands.localAddress();
    } catch {
      localIp = null;
    }
  });

  async function add(event: SubmitEvent) {
    event.preventDefault();
    addError = "";
    addedName = "";
    adding = true;
    try {
      const snapshot = await devices.add(ip.trim());
      addedName = snapshot.displayName;
      ip = "";
    } catch (e) {
      addError = errorMessage(e, "Could not reach that address.");
    } finally {
      adding = false;
    }
  }
</script>

<svelte:head><title>Devices — MusicSync</title></svelte:head>

<div class="mx-auto flex max-w-4xl flex-col gap-6">
  <div>
    <h1 class="text-2xl font-semibold text-white">Devices</h1>
    <p class="mt-1 text-sm text-slate-400">
      Add your LP10s by IP address. Automatic discovery arrives in a later milestone.
    </p>
  </div>

  <section
    class="rounded-lg border border-[var(--color-border-subtle)] bg-[var(--color-surface-raised)] p-6"
  >
    <h2 class="text-sm font-medium text-slate-200">Add a device</h2>
    <p class="mt-1 text-sm text-slate-500">
      The address is checked by talking to the device, so it has to be powered on and on
      this network.{#if localIp}
        This computer is at <span class="font-mono text-slate-400">{localIp}</span>.{/if}
    </p>

    <form onsubmit={add} class="mt-4 flex flex-wrap items-start gap-2">
      <input
        bind:value={ip}
        placeholder="192.168.1.42"
        inputmode="decimal"
        autocomplete="off"
        class="w-56 rounded-md border border-[var(--color-border-subtle)] bg-[var(--color-surface)] px-3 py-2 font-mono text-sm text-white outline-none focus:border-[var(--color-accent)]"
      />
      <button
        type="submit"
        disabled={adding || ip.trim() === ""}
        class="flex items-center gap-2 rounded-md bg-[var(--color-accent)] px-4 py-2 text-sm font-medium text-slate-900 transition-opacity hover:opacity-90 disabled:opacity-50"
      >
        {#if adding}
          <span
            class="size-3.5 animate-spin rounded-full border-2 border-slate-900/30 border-t-slate-900"
            aria-hidden="true"
          ></span>
          Checking…
        {:else}
          Add device
        {/if}
      </button>
    </form>

    {#if addError}
      <p class="mt-3 rounded-md bg-red-500/10 px-3 py-2 text-sm text-red-300">{addError}</p>
    {:else if addedName}
      <p class="mt-3 rounded-md bg-emerald-500/10 px-3 py-2 text-sm text-emerald-300">
        Added {addedName}.
      </p>
    {/if}
  </section>

  <section class="flex flex-col gap-3">
    <div class="flex items-baseline justify-between">
      <h2 class="text-sm font-medium text-slate-200">
        Your devices {#if devices.count > 0}<span class="text-slate-500"
            >({devices.online.length}/{devices.count} online)</span
          >{/if}
      </h2>
    </div>

    {#if devices.loading}
      <p class="text-sm text-slate-500">Loading…</p>
    {:else if devices.error}
      <p class="rounded-md bg-red-500/10 px-3 py-2 text-sm text-red-300">{devices.error}</p>
    {:else if devices.count === 0}
      <div
        class="rounded-lg border border-dashed border-[var(--color-border-subtle)] p-6 text-sm text-slate-500"
      >
        No devices yet. Add one by IP above — you'll find the address in your router's client
        list or in the Arylic app.
      </div>
    {:else}
      <ul class="flex flex-col gap-3">
        {#each devices.list as device (device.uuid)}
          <DeviceCard {device} />
        {/each}
      </ul>
    {/if}
  </section>
</div>
