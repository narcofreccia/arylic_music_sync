<script lang="ts">
  import { devices } from "$lib/stores/devices.svelte";
  import { stream, MAX_DELAY_MS } from "$lib/stores/stream.svelte";
  import { errorMessage } from "$lib/tauri/commands";
  import NetBadge from "$lib/components/device/NetBadge.svelte";

  // Speaker group picker: discovered LP10s *and* manually-added RAOP receivers
  // (Feature 1) as checkboxes that build the stream group. `selected` is a
  // bindable list of keys — a device UUID for discovered speakers, a manual-id
  // for manual ones — owned by the page.
  //
  // Each speaker also gets an "Advanced" disclosure with a per-device delay
  // slider (Feature 2), so offsets can be pre-tuned before streaming; the value
  // is persisted by key and re-applied on the next start.

  let { selected = $bindable([]), disabled = false }: { selected: string[]; disabled?: boolean } =
    $props();

  /** Which speakers have their delay tuner expanded, keyed by delay key. */
  let expanded = $state<Record<string, boolean>>({});

  // --- add-by-IP form ---
  let name = $state("");
  let ip = $state("");
  let port = $state(5000);
  let addingManual = $state(false);
  let manualError = $state("");
  let showHelp = $state(false);

  function toggle(key: string) {
    selected = selected.includes(key)
      ? selected.filter((k) => k !== key)
      : [...selected, key];
  }

  function toggleAdvanced(key: string) {
    expanded = { ...expanded, [key]: !expanded[key] };
  }

  function onDelay(key: string, event: Event) {
    const ms = Number((event.currentTarget as HTMLInputElement).value);
    stream.setTargetDelay(key, ms);
  }

  async function addManual(event: SubmitEvent) {
    event.preventDefault();
    manualError = "";
    addingManual = true;
    try {
      await stream.addManualTarget(name.trim(), ip.trim(), port);
      name = "";
      ip = "";
      port = 5000;
    } catch (e) {
      manualError = errorMessage(e, "Could not add that speaker.");
    } finally {
      addingManual = false;
    }
  }

  async function removeManual(id: string) {
    // Drop it from the current selection too, so a stale key can't be streamed.
    selected = selected.filter((k) => k !== id);
    await stream.removeManualTarget(id);
  }
</script>

{#snippet delayTuner(key: string, label: string)}
  {@const ms = stream.getDelay(key)}
  <div class="mt-2 border-t border-[var(--color-border-subtle)] pt-2">
    <div class="flex items-center gap-3">
      <label class="w-14 shrink-0 text-xs text-slate-400" for="predelay-{key}">Delay</label>
      <input
        id="predelay-{key}"
        type="range"
        min="0"
        max={MAX_DELAY_MS}
        step="10"
        value={ms}
        oninput={(e) => onDelay(key, e)}
        aria-label="{label} delay in milliseconds"
        class="h-1.5 w-full cursor-pointer appearance-none rounded-full bg-[var(--color-surface-raised)] accent-[var(--color-accent)]"
      />
      <span class="w-12 shrink-0 text-right font-mono text-xs text-slate-400 tabular-nums"
        >{ms}ms</span
      >
    </div>
    <p class="mt-1 pl-[4.25rem] text-[11px] text-slate-500">
      Silent lead-in added ahead of this speaker to trim room-to-room skew.
    </p>
  </div>
{/snippet}

<div class="flex flex-col gap-4">
  {#if devices.loading}
    <p class="text-sm text-slate-500">Loading speakers…</p>
  {:else}
    {#if devices.count === 0 && stream.manualTargets.length === 0}
      <div
        class="rounded-lg border border-dashed border-[var(--color-border-subtle)] p-6 text-sm text-slate-500"
      >
        No speakers yet. <a
          href="/devices"
          class="text-[var(--color-accent)] underline underline-offset-2">Scan for speakers</a
        >, or add one by IP below — including a local test receiver.
      </div>
    {/if}

    <!-- Discovered LP10s -->
    {#if devices.count > 0}
      <ul class="flex flex-col gap-2">
        {#each devices.list as device (device.uuid)}
          {@const checked = selected.includes(device.uuid)}
          {@const selectable = device.online && !disabled}
          <li
            class="rounded-md border border-[var(--color-border-subtle)] bg-[var(--color-surface)] {selectable
              ? ''
              : 'opacity-50'}"
          >
            <label
              class="flex items-center gap-3 px-3 py-2 {selectable
                ? 'cursor-pointer'
                : 'cursor-not-allowed'}"
            >
              <input
                type="checkbox"
                {checked}
                disabled={disabled || !device.online}
                onchange={() => toggle(device.uuid)}
                class="size-4 shrink-0 accent-[var(--color-accent)]"
              />
              <span class="min-w-0 flex-1 truncate text-sm text-slate-200">{device.displayName}</span
              >
              <span class="font-mono text-xs text-slate-500">{device.ip}</span>
              {#if device.online}
                <NetBadge netMode={device.netMode} wifiBand={device.wifiBand} />
              {:else}
                <span class="text-xs text-slate-500">offline</span>
              {/if}
            </label>
            <div class="px-3 pb-2">
              <button
                type="button"
                onclick={() => toggleAdvanced(device.uuid)}
                aria-expanded={expanded[device.uuid] ?? false}
                class="flex items-center gap-1.5 text-xs text-slate-500 transition-colors hover:text-slate-300"
              >
                <span class="inline-block transition-transform {expanded[device.uuid] ? 'rotate-90' : ''}"
                  >›</span
                >
                Advanced{#if stream.getDelay(device.uuid) > 0}
                  <span class="text-slate-400">· {stream.getDelay(device.uuid)}ms delay</span>
                {/if}
              </button>
              {#if expanded[device.uuid]}
                {@render delayTuner(device.uuid, device.displayName)}
              {/if}
            </div>
          </li>
        {/each}
      </ul>
    {/if}

    <!-- Manual / test receivers (Feature 1) -->
    {#if stream.manualTargets.length > 0}
      <ul class="flex flex-col gap-2">
        {#each stream.manualTargets as target (target.id)}
          {@const checked = selected.includes(target.id)}
          <li class="rounded-md border border-[var(--color-border-subtle)] bg-[var(--color-surface)]">
            <div class="flex items-center gap-3 px-3 py-2">
              <label class="flex min-w-0 flex-1 cursor-pointer items-center gap-3">
                <input
                  type="checkbox"
                  {checked}
                  {disabled}
                  onchange={() => toggle(target.id)}
                  class="size-4 shrink-0 accent-[var(--color-accent)]"
                />
                <span class="min-w-0 flex-1 truncate text-sm text-slate-200">{target.name}</span>
                <span class="font-mono text-xs text-slate-500">{target.ip}:{target.port}</span>
                <span
                  class="rounded bg-[var(--color-surface-raised)] px-1.5 py-0.5 text-[10px] font-medium tracking-wide text-slate-400 uppercase"
                  >manual</span
                >
              </label>
              <button
                type="button"
                onclick={() => removeManual(target.id)}
                aria-label="Remove {target.name}"
                title="Remove"
                class="shrink-0 rounded p-1 text-slate-500 transition-colors hover:bg-[var(--color-surface-raised)] hover:text-red-300"
              >
                <span aria-hidden="true">✕</span>
              </button>
            </div>
            <div class="px-3 pb-2">
              <button
                type="button"
                onclick={() => toggleAdvanced(target.id)}
                aria-expanded={expanded[target.id] ?? false}
                class="flex items-center gap-1.5 text-xs text-slate-500 transition-colors hover:text-slate-300"
              >
                <span class="inline-block transition-transform {expanded[target.id] ? 'rotate-90' : ''}"
                  >›</span
                >
                Advanced{#if stream.getDelay(target.id) > 0}
                  <span class="text-slate-400">· {stream.getDelay(target.id)}ms delay</span>
                {/if}
              </button>
              {#if expanded[target.id]}
                {@render delayTuner(target.id, target.name)}
              {/if}
            </div>
          </li>
        {/each}
      </ul>
    {/if}

    <!-- Add a speaker by IP (Feature 1) -->
    <div class="rounded-md border border-[var(--color-border-subtle)] bg-[var(--color-surface)] p-3">
      <div class="flex items-center gap-2">
        <h3 class="text-xs font-medium text-slate-300">Add a speaker by IP</h3>
        <button
          type="button"
          onclick={() => (showHelp = !showHelp)}
          aria-expanded={showHelp}
          aria-label="How to run a local test receiver"
          class="flex size-4 items-center justify-center rounded-full border border-[var(--color-border-subtle)] text-[10px] text-slate-400 transition-colors hover:text-slate-200"
          >?</button
        >
      </div>

      {#if showHelp}
        <div
          class="mt-2 rounded-md border border-[var(--color-border-subtle)] bg-[var(--color-surface-raised)] p-3 text-xs text-slate-400"
        >
          <p>
            Test "Play Everywhere" without real speakers by pointing at a local AirPlay/RAOP
            receiver:
          </p>
          <ol class="mt-2 list-decimal space-y-1 pl-4">
            <li>
              Install one: <code class="rounded bg-[var(--color-surface)] px-1 py-0.5 font-mono text-slate-300"
                >brew install shairport-sync</code
              >
            </li>
            <li>
              Run it: <code class="rounded bg-[var(--color-surface)] px-1 py-0.5 font-mono text-slate-300"
                >shairport-sync -a "Test Room" --port=5000</code
              >
            </li>
            <li>Add it here as <span class="font-mono text-slate-300">127.0.0.1 : 5000</span>.</li>
          </ol>
          <p class="mt-2">
            macOS's own AirPlay Receiver (Control Center) also binds port 5000. If shairport-sync
            can't start, use a different port (e.g. <span class="font-mono text-slate-300">5001</span
            >) or turn off the Mac's AirPlay Receiver in System Settings › General › AirDrop &amp;
            Handoff.
          </p>
        </div>
      {/if}

      <form onsubmit={addManual} class="mt-3 flex flex-wrap items-end gap-2">
        <label class="flex flex-col gap-1 text-[11px] text-slate-500">
          Name
          <input
            bind:value={name}
            placeholder="Test Room"
            autocomplete="off"
            class="w-36 rounded-md border border-[var(--color-border-subtle)] bg-[var(--color-surface-raised)] px-2 py-1.5 text-sm text-white outline-none focus:border-[var(--color-accent)]"
          />
        </label>
        <label class="flex flex-col gap-1 text-[11px] text-slate-500">
          IP address
          <input
            bind:value={ip}
            placeholder="127.0.0.1"
            inputmode="decimal"
            autocomplete="off"
            spellcheck="false"
            class="w-32 rounded-md border border-[var(--color-border-subtle)] bg-[var(--color-surface-raised)] px-2 py-1.5 font-mono text-sm text-white outline-none focus:border-[var(--color-accent)]"
          />
        </label>
        <label class="flex flex-col gap-1 text-[11px] text-slate-500">
          Port
          <input
            type="number"
            bind:value={port}
            min="1"
            max="65535"
            class="w-20 rounded-md border border-[var(--color-border-subtle)] bg-[var(--color-surface-raised)] px-2 py-1.5 font-mono text-sm text-white outline-none focus:border-[var(--color-accent)]"
          />
        </label>
        <button
          type="submit"
          disabled={addingManual || name.trim() === "" || ip.trim() === ""}
          class="rounded-md bg-[var(--color-accent)] px-3 py-1.5 text-sm font-medium text-slate-900 transition-opacity hover:opacity-90 disabled:opacity-50"
        >
          {addingManual ? "Adding…" : "Add speaker"}
        </button>
      </form>

      {#if manualError}
        <p class="mt-2 rounded-md bg-red-500/10 px-3 py-2 text-sm text-red-300">{manualError}</p>
      {/if}
    </div>
  {/if}
</div>
