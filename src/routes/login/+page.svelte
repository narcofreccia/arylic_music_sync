<script lang="ts">
  import { goto } from "$app/navigation";
  import { session } from "$lib/stores/session.svelte";
  import { errorCode, errorMessage } from "$lib/tauri/commands";

  // FR-2: the login screen for every launch after the first, unless "remember
  // me" already opened the session (the layout guard never routes us here then).
  let password = $state("");
  let rememberMe = $state(false);
  let error = $state("");
  let busy = $state(false);
  /** Lockout is a throttle response, not a typo — surfaced more prominently. */
  let lockedOut = $state(false);

  let usernameInput = $state("");
  const username = $derived(session.username ?? usernameInput);

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    error = "";
    lockedOut = false;
    busy = true;
    try {
      const ok = await session.login(username, password, rememberMe);
      if (ok) {
        password = "";
        await goto("/");
      } else {
        error = "Wrong password or PIN.";
        password = "";
      }
    } catch (e) {
      lockedOut = errorCode(e) === "locked_out";
      error = errorMessage(e, "Could not sign in.");
    } finally {
      busy = false;
    }
  }
</script>

<svelte:head><title>Sign in — MusicSync</title></svelte:head>

<form
  onsubmit={submit}
  class="w-full max-w-sm rounded-xl border border-[var(--color-border-subtle)] bg-[var(--color-surface-raised)] p-8 shadow-xl"
>
  <h1 class="text-xl font-semibold text-white">Welcome back</h1>
  <p class="mt-2 text-sm text-slate-400">Unlock MusicSync to control your LP10s.</p>

  <div class="mt-6 flex flex-col gap-4">
    {#if session.username}
      <!-- The profile is single-user, so the username is a label, not a field. -->
      <div class="flex flex-col gap-1.5">
        <span class="text-xs font-medium text-slate-300">Profile</span>
        <div
          class="rounded-md border border-[var(--color-border-subtle)] bg-[var(--color-surface)] px-3 py-2 text-sm text-slate-400"
        >
          {session.username}
        </div>
      </div>
    {:else}
      <label class="flex flex-col gap-1.5">
        <span class="text-xs font-medium text-slate-300">Username</span>
        <input
          bind:value={usernameInput}
          autocomplete="username"
          spellcheck="false"
          class="rounded-md border border-[var(--color-border-subtle)] bg-[var(--color-surface)] px-3 py-2 text-sm text-white outline-none focus:border-[var(--color-accent)]"
        />
      </label>
    {/if}

    <label class="flex flex-col gap-1.5">
      <span class="text-xs font-medium text-slate-300">Password or PIN</span>
      <!-- svelte-ignore a11y_autofocus -->
      <input
        bind:value={password}
        autofocus
        type="password"
        autocomplete="current-password"
        class="rounded-md border border-[var(--color-border-subtle)] bg-[var(--color-surface)] px-3 py-2 text-sm text-white outline-none focus:border-[var(--color-accent)]"
      />
    </label>

    <label class="flex items-center gap-2 text-sm text-slate-300">
      <input
        bind:checked={rememberMe}
        type="checkbox"
        class="size-4 accent-[var(--color-accent)]"
      />
      Remember me on this machine
    </label>
  </div>

  {#if error}
    <p
      class="mt-4 rounded-md px-3 py-2 text-sm {lockedOut
        ? 'bg-amber-500/10 text-amber-300'
        : 'bg-red-500/10 text-red-300'}"
    >
      {error}
    </p>
  {/if}

  <button
    type="submit"
    disabled={busy}
    class="mt-6 w-full rounded-md bg-[var(--color-accent)] px-4 py-2 text-sm font-medium text-slate-900 transition-opacity hover:opacity-90 disabled:opacity-50"
  >
    {busy ? "Signing in…" : "Sign in"}
  </button>
</form>
