<script lang="ts">
  import { onMount } from "svelte";
  import { goto } from "$app/navigation";
  import { session } from "$lib/stores/session.svelte";
  import { commands, errorMessage } from "$lib/tauri/commands";

  // FR-3: change/remove the local password, plus FR-20's scan subnet (the one
  // preference M3 needs). Polling interval, theme and Group Guard land with M6.

  let current = $state("");
  let next = $state("");
  let confirm = $state("");
  let pwBusy = $state(false);
  let pwError = $state("");
  let pwDone = $state("");

  let removeConfirming = $state(false);
  let removeCurrent = $state("");
  let removeBusy = $state(false);
  let removeError = $state("");

  let rememberBusy = $state(false);

  // ------------------------------------------------- network (FR-4 / FR-20) --

  let subnet = $state("");
  let subnetBusy = $state(false);
  let subnetError = $state("");
  let subnetDone = $state("");
  /** This machine's LAN address — what auto-detection would pick. */
  let autoCidr = $state("");

  onMount(async () => {
    try {
      subnet = (await commands.getSettings()).subnet ?? "";
    } catch (e) {
      subnetError = errorMessage(e, "Could not read your settings.");
    }
    try {
      const local = await commands.localAddress();
      autoCidr = local ? `${local.split(".").slice(0, 3).join(".")}.0/24` : "";
    } catch {
      autoCidr = "";
    }
  });

  async function saveSubnet(event: SubmitEvent) {
    event.preventDefault();
    subnetError = "";
    subnetDone = "";
    subnetBusy = true;
    try {
      // Empty means "go back to auto-detection"; the Rust side validates the
      // rest (including the /16 width cap) so the two can't disagree.
      const value = subnet.trim();
      const saved = await commands.setSubnet(value === "" ? null : value);
      subnet = saved.subnet ?? "";
      subnetDone = saved.subnet ? `Scans will sweep ${saved.subnet}.` : "Back to auto-detection.";
    } catch (e) {
      subnetError = errorMessage(e, "Could not save that range.");
    } finally {
      subnetBusy = false;
    }
  }

  async function changePassword(event: SubmitEvent) {
    event.preventDefault();
    pwError = "";
    pwDone = "";
    if (next.length < 4) {
      pwError = "Password or PIN must be at least 4 characters.";
      return;
    }
    if (next !== confirm) {
      pwError = "The two entries don't match.";
      return;
    }

    pwBusy = true;
    try {
      await session.setPassword(current, next);
      current = next = confirm = "";
      pwDone = session.hasPassword ? "Password updated." : "Password set.";
      await session.refresh();
    } catch (e) {
      pwError = errorMessage(e, "Could not change the password.");
    } finally {
      pwBusy = false;
    }
  }

  async function removePassword(event: SubmitEvent) {
    event.preventDefault();
    removeError = "";
    removeBusy = true;
    try {
      await session.removePassword(removeCurrent);
      removeCurrent = "";
      removeConfirming = false;
    } catch (e) {
      removeError = errorMessage(e, "Could not remove the password.");
    } finally {
      removeBusy = false;
    }
  }

  async function toggleRemember(event: Event) {
    const value = (event.currentTarget as HTMLInputElement).checked;
    rememberBusy = true;
    try {
      await session.setRememberMe(value);
    } finally {
      rememberBusy = false;
    }
  }

  async function signOut() {
    await session.logout();
    await goto("/login");
  }

  const field =
    "rounded-md border border-[var(--color-border-subtle)] bg-[var(--color-surface)] px-3 py-2 text-sm text-white outline-none focus:border-[var(--color-accent)]";
</script>

<svelte:head><title>Settings — MusicSync</title></svelte:head>

<div class="mx-auto flex max-w-2xl flex-col gap-6">
  <div>
    <h1 class="text-2xl font-semibold text-white">Settings</h1>
    <p class="mt-1 text-sm text-slate-400">
      Polling, theme and Group Guard options arrive with later milestones.
    </p>
  </div>

  <!-- FR-20: the default range for the discovery sweep (FR-4). -->
  <section
    class="rounded-lg border border-[var(--color-border-subtle)] bg-[var(--color-surface-raised)] p-6"
  >
    <h2 class="text-sm font-medium text-slate-200">Network</h2>
    <p class="mt-1 text-sm text-slate-500">
      Which range "Scan network" sweeps when you don't override it on the Devices page.
      Leave it empty to detect it from this computer's address{#if autoCidr}
        (<span class="font-mono text-slate-400">{autoCidr}</span>){/if}.
    </p>

    <form onsubmit={saveSubnet} class="mt-4 flex flex-wrap items-start gap-2">
      <input
        bind:value={subnet}
        placeholder={autoCidr || "192.168.1.0/24"}
        autocomplete="off"
        spellcheck="false"
        class="{field} w-56 font-mono"
      />
      <button
        type="submit"
        disabled={subnetBusy}
        class="rounded-md bg-[var(--color-accent)] px-4 py-2 text-sm font-medium text-slate-900 transition-opacity hover:opacity-90 disabled:opacity-50"
      >
        {subnetBusy ? "Saving…" : "Save"}
      </button>
    </form>

    {#if subnetError}
      <p class="mt-3 rounded-md bg-red-500/10 px-3 py-2 text-sm text-red-300">{subnetError}</p>
    {:else if subnetDone}
      <p class="mt-3 rounded-md bg-emerald-500/10 px-3 py-2 text-sm text-emerald-300">
        {subnetDone}
      </p>
    {/if}
  </section>

  <section
    class="rounded-lg border border-[var(--color-border-subtle)] bg-[var(--color-surface-raised)] p-6"
  >
    <h2 class="text-sm font-medium text-slate-200">Account</h2>
    <p class="mt-1 text-sm text-slate-500">
      Signed in as <span class="text-slate-300">{session.username ?? "—"}</span>. Your
      password is stored on this machine only, as an Argon2 hash.
    </p>

    <label class="mt-5 flex items-center gap-2 text-sm text-slate-300">
      <input
        type="checkbox"
        checked={session.rememberMe}
        disabled={rememberBusy}
        onchange={toggleRemember}
        class="size-4 accent-[var(--color-accent)]"
      />
      Remember me on this machine (skip the login screen)
    </label>

    <!-- Change password -->
    <form onsubmit={changePassword} class="mt-6 flex flex-col gap-3 border-t border-[var(--color-border-subtle)] pt-6">
      <h3 class="text-xs font-medium tracking-wide text-slate-400 uppercase">
        {session.hasPassword ? "Change password or PIN" : "Set a password or PIN"}
      </h3>

      {#if session.hasPassword}
        <input
          bind:value={current}
          type="password"
          autocomplete="current-password"
          placeholder="Current password"
          class={field}
        />
      {/if}
      <input
        bind:value={next}
        type="password"
        autocomplete="new-password"
        placeholder="New password"
        class={field}
      />
      <input
        bind:value={confirm}
        type="password"
        autocomplete="new-password"
        placeholder="Confirm new password"
        class={field}
      />

      {#if pwError}
        <p class="rounded-md bg-red-500/10 px-3 py-2 text-sm text-red-300">{pwError}</p>
      {:else if pwDone}
        <p class="rounded-md bg-emerald-500/10 px-3 py-2 text-sm text-emerald-300">{pwDone}</p>
      {/if}

      <button
        type="submit"
        disabled={pwBusy}
        class="self-start rounded-md bg-[var(--color-accent)] px-4 py-2 text-sm font-medium text-slate-900 transition-opacity hover:opacity-90 disabled:opacity-50"
      >
        {pwBusy ? "Saving…" : "Save password"}
      </button>
    </form>

    <!-- Remove password -->
    <div class="mt-6 border-t border-[var(--color-border-subtle)] pt-6">
      <h3 class="text-xs font-medium tracking-wide text-slate-400 uppercase">Remove password</h3>

      {#if !session.hasPassword}
        <p class="mt-2 text-sm text-slate-500">
          No password is set — MusicSync opens straight to the dashboard.
        </p>
      {:else if !removeConfirming}
        <p class="mt-2 text-sm text-slate-500">
          Anyone using this computer will be able to open MusicSync without signing in.
        </p>
        <button
          type="button"
          onclick={() => {
            removeConfirming = true;
            removeError = "";
          }}
          class="mt-3 rounded-md border border-red-500/40 px-4 py-2 text-sm font-medium text-red-300 transition-colors hover:bg-red-500/10"
        >
          Remove password
        </button>
      {:else}
        <!-- Inline confirm: an in-window step rather than a native dialog. -->
        <form onsubmit={removePassword} class="mt-3 flex flex-col gap-3">
          <p class="text-sm text-slate-300">
            Confirm with your current password to remove it.
          </p>
          <input
            bind:value={removeCurrent}
            type="password"
            autocomplete="current-password"
            placeholder="Current password"
            class={field}
          />
          {#if removeError}
            <p class="rounded-md bg-red-500/10 px-3 py-2 text-sm text-red-300">{removeError}</p>
          {/if}
          <div class="flex gap-2">
            <button
              type="submit"
              disabled={removeBusy}
              class="rounded-md bg-red-500/80 px-4 py-2 text-sm font-medium text-white transition-opacity hover:opacity-90 disabled:opacity-50"
            >
              {removeBusy ? "Removing…" : "Yes, remove it"}
            </button>
            <button
              type="button"
              onclick={() => {
                removeConfirming = false;
                removeCurrent = "";
                removeError = "";
              }}
              class="rounded-md border border-[var(--color-border-subtle)] px-4 py-2 text-sm text-slate-300 transition-colors hover:bg-[var(--color-surface)]"
            >
              Cancel
            </button>
          </div>
        </form>
      {/if}
    </div>

    {#if session.hasPassword}
      <div class="mt-6 border-t border-[var(--color-border-subtle)] pt-6">
        <button
          type="button"
          onclick={signOut}
          class="rounded-md border border-[var(--color-border-subtle)] px-4 py-2 text-sm text-slate-300 transition-colors hover:bg-[var(--color-surface)]"
        >
          Sign out
        </button>
      </div>
    {/if}
  </section>
</div>
