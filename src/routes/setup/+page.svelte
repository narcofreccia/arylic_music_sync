<script lang="ts">
  import { goto } from "$app/navigation";
  import { session } from "$lib/stores/session.svelte";
  import { errorMessage } from "$lib/tauri/commands";

  // FR-1: first-run wizard. One local profile, hashed with Argon2 on the Rust
  // side — nothing leaves the machine.
  let username = $state("");
  let password = $state("");
  let confirm = $state("");
  let rememberMe = $state(true);
  let error = $state("");
  let busy = $state(false);

  /** Client-side check only; Rust re-validates before hashing. */
  function localError(): string {
    if (!username.trim()) return "Please choose a username.";
    if (password.length < 4) return "Password or PIN must be at least 4 characters.";
    if (password !== confirm) return "The two entries don't match.";
    return "";
  }

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    error = localError();
    if (error) return;

    busy = true;
    try {
      await session.createProfile(username, password, rememberMe);
      await goto("/");
    } catch (e) {
      error = errorMessage(e, "Could not create the profile.");
    } finally {
      busy = false;
    }
  }
</script>

<svelte:head><title>Set up MusicSync</title></svelte:head>

<form
  onsubmit={submit}
  class="w-full max-w-sm rounded-xl border border-[var(--color-border-subtle)] bg-[var(--color-surface-raised)] p-8 shadow-xl"
>
  <h1 class="text-xl font-semibold text-white">Welcome to MusicSync</h1>
  <p class="mt-2 text-sm text-slate-400">
    Create a local profile to protect this app. It stays on this machine — there is no
    account and no cloud.
  </p>

  <div class="mt-6 flex flex-col gap-4">
    <label class="flex flex-col gap-1.5">
      <span class="text-xs font-medium text-slate-300">Username</span>
      <!-- svelte-ignore a11y_autofocus -->
      <input
        bind:value={username}
        autofocus
        autocomplete="username"
        spellcheck="false"
        class="rounded-md border border-[var(--color-border-subtle)] bg-[var(--color-surface)] px-3 py-2 text-sm text-white outline-none focus:border-[var(--color-accent)]"
      />
    </label>

    <label class="flex flex-col gap-1.5">
      <span class="text-xs font-medium text-slate-300">Password or PIN</span>
      <input
        bind:value={password}
        type="password"
        autocomplete="new-password"
        class="rounded-md border border-[var(--color-border-subtle)] bg-[var(--color-surface)] px-3 py-2 text-sm text-white outline-none focus:border-[var(--color-accent)]"
      />
      <span class="text-xs text-slate-500">At least 4 characters — a PIN is fine.</span>
    </label>

    <label class="flex flex-col gap-1.5">
      <span class="text-xs font-medium text-slate-300">Confirm</span>
      <input
        bind:value={confirm}
        type="password"
        autocomplete="new-password"
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
    <p class="mt-4 rounded-md bg-red-500/10 px-3 py-2 text-sm text-red-300">{error}</p>
  {/if}

  <button
    type="submit"
    disabled={busy}
    class="mt-6 w-full rounded-md bg-[var(--color-accent)] px-4 py-2 text-sm font-medium text-slate-900 transition-opacity hover:opacity-90 disabled:opacity-50"
  >
    {busy ? "Creating…" : "Create profile"}
  </button>
</form>
