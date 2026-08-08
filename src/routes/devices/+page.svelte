<script lang="ts">
  import { onMount } from "svelte";
  import DeviceCard from "$lib/components/device/DeviceCard.svelte";
  import NetBadge from "$lib/components/device/NetBadge.svelte";
  import { devices } from "$lib/stores/devices.svelte";
  import { scan, candidateKey } from "$lib/stores/scan.svelte";
  import { commands, errorMessage } from "$lib/tauri/commands";
  import type { DeviceCandidate, ScanPhase } from "$lib/types";

  // Devices page (FR-4 … FR-9): network discovery, manual add, and the list.
  //
  // Event listeners are registered once by the root layout via devices.start()
  // and scan.start(); this page only reads the stores, so navigating away and
  // back doesn't stack subscriptions.

  let ip = $state("");
  let adding = $state(false);
  let addError = $state("");
  let addedName = $state("");

  /** This machine's LAN address, purely as a hint for what to type. */
  let localIp = $state<string | null>(null);

  // ------------------------------------------------------------- scan (FR-4) --

  let sweep = $state(true);
  let cidr = $state("");
  let advanced = $state(false);
  /** The saved default (FR-20), shown as the placeholder when it is set. */
  let savedSubnet = $state<string | null>(null);
  /** Per-candidate add state, so one slow device doesn't disable every row. */
  let addingKey = $state<string | null>(null);
  let candidateError = $state("");

  const PHASE_LABEL: Record<ScanPhase, string> = {
    ddms: "Listening for DDMS speakers",
    ssdp: "Asking over SSDP",
    sweep: "Sweeping the subnet",
  };

  /** The /24 the sweep falls back to when nothing is configured. */
  const autoCidr = $derived(
    localIp ? `${localIp.split(".").slice(0, 3).join(".")}.0/24` : ""
  );
  const cidrPlaceholder = $derived(savedSubnet || autoCidr || "192.168.1.0/24");

  /**
   * Loose client-side shape check only — the authoritative validation (and the
   * /16 width cap) lives in Rust, and rejecting here what Rust would accept is
   * how the two drift apart.
   */
  const cidrValid = $derived(cidr.trim() === "" || /^\d{1,3}(\.\d{1,3}){3}\/\d{1,2}$/.test(cidr.trim()));

  const busy = $derived(scan.running || addingKey !== null);

  onMount(async () => {
    try {
      localIp = await commands.localAddress();
    } catch {
      localIp = null;
    }
    try {
      savedSubnet = (await commands.getSettings()).subnet;
    } catch {
      savedSubnet = null;
    }
  });

  async function startScan() {
    if (!cidrValid) return;
    candidateError = "";
    await scan.scan({ sweep, cidr: cidr.trim() === "" ? null : cidr.trim() });
  }

  /** Candidates go through the normal add path (FR-5), never straight to disk. */
  async function addCandidate(candidate: DeviceCandidate) {
    addingKey = candidateKey(candidate);
    candidateError = "";
    try {
      await devices.add(candidate.ip);
      scan.markAdded(candidate);
    } catch (e) {
      candidateError = errorMessage(e, `Could not add ${candidate.ip}.`);
    } finally {
      addingKey = null;
    }
  }

  /** Sequential on purpose: adding starts a poll task per device. */
  async function addAllNew() {
    for (const candidate of scan.newCandidates) {
      await addCandidate(candidate);
    }
  }

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
      Scan your network for LP10s, or add one by IP address.
    </p>
  </div>

  <!-- FR-4: mDNS + SSDP + optional subnet sweep. -->
  <section
    class="rounded-lg border border-[var(--color-border-subtle)] bg-[var(--color-surface-raised)] p-6"
  >
    <div class="flex flex-wrap items-start justify-between gap-3">
      <div>
        <h2 class="text-sm font-medium text-slate-200">Find devices</h2>
        <p class="mt-1 text-sm text-slate-500">
          Listens for mDNS and SSDP announcements, and can probe every address on your
          subnet. Everything found is checked before it's offered.
        </p>
      </div>

      {#if scan.running}
        <button
          type="button"
          onclick={() => scan.cancel()}
          class="rounded-md border border-[var(--color-border-subtle)] px-4 py-2 text-sm text-slate-300 transition-colors hover:bg-[var(--color-surface)]"
        >
          Cancel
        </button>
      {:else}
        <button
          type="button"
          onclick={startScan}
          disabled={!cidrValid || busy}
          class="rounded-md bg-[var(--color-accent)] px-4 py-2 text-sm font-medium text-slate-900 transition-opacity hover:opacity-90 disabled:opacity-50"
        >
          Scan network
        </button>
      {/if}
    </div>

    {#if scan.running}
      <div class="mt-4 flex items-center gap-2 text-sm text-slate-400">
        <span
          class="size-3.5 animate-spin rounded-full border-2 border-slate-600 border-t-[var(--color-accent)]"
          aria-hidden="true"
        ></span>
        <span>{scan.phase ? PHASE_LABEL[scan.phase] : "Starting…"}</span>
        {#if scan.total > 0}
          <span class="font-mono text-xs text-slate-500">{scan.scanned}/{scan.total}</span>
        {/if}
        <span class="ml-auto text-xs text-slate-500">
          {scan.found.length}
          {scan.found.length === 1 ? "device" : "devices"} found
        </span>
      </div>

      <!-- Determinate only for the sweep; mDNS/SSDP have no denominator. -->
      <div
        class="mt-2 h-1.5 overflow-hidden rounded-full bg-[var(--color-surface)]"
        role="progressbar"
        aria-valuemin="0"
        aria-valuemax={scan.total || undefined}
        aria-valuenow={scan.total > 0 ? scan.scanned : undefined}
      >
        {#if scan.progress !== null}
          <div
            class="h-full rounded-full bg-[var(--color-accent)] transition-[width] duration-200"
            style="width: {Math.round(scan.progress * 100)}%"
          ></div>
        {:else}
          <div class="h-full w-1/3 animate-pulse rounded-full bg-[var(--color-accent)]/60"></div>
        {/if}
      </div>
    {/if}

    <!-- Advanced: the sweep toggle and its range (FR-4 "optional", FR-20). -->
    <button
      type="button"
      onclick={() => (advanced = !advanced)}
      aria-expanded={advanced}
      class="mt-4 flex items-center gap-1.5 text-xs text-slate-500 transition-colors hover:text-slate-300"
    >
      <span class="inline-block transition-transform {advanced ? 'rotate-90' : ''}">›</span>
      Advanced
    </button>

    {#if advanced}
      <div class="mt-3 flex flex-wrap items-center gap-x-6 gap-y-3 border-t border-[var(--color-border-subtle)] pt-4">
        <label class="flex items-center gap-2 text-sm text-slate-300">
          <input
            type="checkbox"
            bind:checked={sweep}
            disabled={scan.running}
            class="size-4 accent-[var(--color-accent)]"
          />
          Also probe every address on the subnet
        </label>

        <label class="flex items-center gap-2 text-sm text-slate-300">
          Range
          <input
            bind:value={cidr}
            disabled={!sweep || scan.running}
            placeholder={cidrPlaceholder}
            autocomplete="off"
            spellcheck="false"
            class="w-44 rounded-md border px-3 py-1.5 font-mono text-sm text-white outline-none disabled:opacity-50 {cidrValid
              ? 'border-[var(--color-border-subtle)] focus:border-[var(--color-accent)]'
              : 'border-red-500/60'} bg-[var(--color-surface)]"
          />
        </label>

        {#if !cidrValid}
          <p class="w-full text-sm text-red-300">
            Use CIDR notation, for example <span class="font-mono">192.168.1.0/24</span>.
          </p>
        {:else if sweep}
          <p class="w-full text-xs text-slate-500">
            Leave the range empty to use {savedSubnet
              ? "the subnet saved in Settings"
              : "the network this computer is on"}.
          </p>
        {/if}
      </div>
    {/if}

    {#if scan.error}
      <p class="mt-3 rounded-md bg-red-500/10 px-3 py-2 text-sm text-red-300">{scan.error}</p>
    {/if}
    {#if candidateError}
      <p class="mt-3 rounded-md bg-red-500/10 px-3 py-2 text-sm text-red-300">{candidateError}</p>
    {/if}

    <!-- Results. Candidates are offers, not entries: nothing is saved until
         the user says so, and adding runs the same checks as a manual add. -->
    {#if scan.found.length > 0}
      <div class="mt-5 border-t border-[var(--color-border-subtle)] pt-4">
        <div class="flex flex-wrap items-baseline justify-between gap-2">
          <h3 class="text-xs font-medium tracking-wide text-slate-400 uppercase">
            Found ({scan.found.length})
          </h3>
          <div class="flex items-center gap-2">
            {#if scan.newCandidates.length > 0}
              <button
                type="button"
                onclick={addAllNew}
                disabled={busy}
                class="rounded-md bg-[var(--color-accent)] px-3 py-1.5 text-xs font-medium text-slate-900 transition-opacity hover:opacity-90 disabled:opacity-50"
              >
                Add all new ({scan.newCandidates.length})
              </button>
            {/if}
            <button
              type="button"
              onclick={() => scan.clear()}
              disabled={scan.running}
              class="rounded-md border border-[var(--color-border-subtle)] px-3 py-1.5 text-xs text-slate-400 transition-colors hover:bg-[var(--color-surface)] disabled:opacity-50"
            >
              Clear
            </button>
          </div>
        </div>

        <ul class="mt-3 flex flex-col gap-2">
          {#each scan.found as candidate (candidateKey(candidate))}
            {@const key = candidateKey(candidate)}
            {@const added = candidate.alreadyAdded || scan.added[key]}
            <li
              class="flex flex-wrap items-center gap-x-4 gap-y-1 rounded-md border border-[var(--color-border-subtle)] bg-[var(--color-surface)] px-3 py-2"
            >
              <span class="text-sm text-slate-200">{candidate.name || candidate.ip}</span>
              <span class="font-mono text-xs text-slate-500">{candidate.ip}</span>
              <NetBadge netMode={candidate.netMode} wifiBand={candidate.wifiBand} />
              {#if candidate.model}
                <span class="text-xs text-slate-500">{candidate.model}</span>
              {/if}
              {#if candidate.firmware}
                <span class="text-xs text-slate-500">fw {candidate.firmware}</span>
              {/if}

              <div class="ml-auto">
                {#if added}
                  <span class="text-xs text-slate-500">Added</span>
                {:else}
                  <button
                    type="button"
                    onclick={() => addCandidate(candidate)}
                    disabled={addingKey !== null}
                    class="rounded-md border border-[var(--color-border-subtle)] px-3 py-1 text-xs text-slate-200 transition-colors hover:bg-[var(--color-surface-raised)] disabled:opacity-50"
                  >
                    {addingKey === key ? "Adding…" : "Add"}
                  </button>
                {/if}
              </div>
            </li>
          {/each}
        </ul>
      </div>
    {:else if scan.cancelled}
      <p class="mt-4 text-sm text-slate-500">Scan stopped — nothing found yet.</p>
    {/if}
  </section>

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
        No devices yet. Scan the network above, or add one by IP — you'll find the address in
        your router's client list or in the Arylic app.
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
