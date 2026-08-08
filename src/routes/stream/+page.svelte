<script lang="ts">
  import { stream } from "$lib/stores/stream.svelte";
  import SpotifyNowPlaying from "$lib/components/stream/SpotifyNowPlaying.svelte";
  import StreamGroupPicker from "$lib/components/stream/StreamGroupPicker.svelte";
  import RoomRow from "$lib/components/stream/RoomRow.svelte";

  // "Play Everywhere" (S5): capture Spotify via the MusicSync Connect endpoint
  // and fan the same audio out to the selected LP10s in sync over RAOP.
  //
  // Event listeners are registered once by the root layout via stream.start();
  // this page only reads the store, so navigating away and back doesn't stack
  // subscriptions.

  /** Selected device UUIDs for the group (the picker binds to this). */
  let selected = $state<string[]>([]);

  const spotify = $derived(stream.spotify);
  const playing = $derived(spotify?.play_state === "playing");
  const canTransport = $derived(stream.spotifyConnected);

  // Once streaming, the live group is whatever Rust reports; keep the picker
  // selection in sync (by target key) so stopping and restarting keeps the same
  // rooms — discovered devices and manual targets alike.
  $effect(() => {
    if (stream.streaming) {
      selected = stream.targets.map((t) => t.key);
    }
  });
</script>

<svelte:head><title>Play Everywhere — MusicSync</title></svelte:head>

<div class="mx-auto flex max-w-3xl flex-col gap-6">
  <div>
    <h1 class="text-2xl font-semibold text-white">Play Everywhere</h1>
    <p class="mt-1 text-sm text-slate-400">
      Stream Spotify to all your LP10 speakers at once, in sync.
    </p>
  </div>

  <!-- Honest UX copy: what this needs and what it does. -->
  <div
    class="rounded-lg border border-[var(--color-border-subtle)] bg-[var(--color-surface-raised)] px-4 py-3 text-xs text-slate-400"
  >
    Needs <span class="font-medium text-slate-300">Spotify Premium</span>. Grouping streams the same
    audio to every selected speaker in sync over AirPlay (RAOP); use the per-room delay to trim any
    room-to-room skew.
  </div>

  <!-- ------------------------------------------------------- Spotify panel -- -->
  <section
    class="rounded-lg border border-[var(--color-border-subtle)] bg-[var(--color-surface-raised)] p-6"
  >
    <div class="flex flex-wrap items-start justify-between gap-3">
      <div>
        <h2 class="text-sm font-medium text-slate-200">Spotify capture</h2>
        <p class="mt-1 flex items-center gap-2 text-sm text-slate-500">
          {#if stream.spotifyConnected}
            <span class="size-2 rounded-full bg-emerald-400" aria-hidden="true"></span>
            Spotify is connected to MusicSync.
          {:else if stream.spotifyRunning}
            <span class="size-2 rounded-full bg-amber-400" aria-hidden="true"></span>
            Advertising — pick <span class="font-medium text-slate-300">MusicSync</span> in Spotify.
          {:else}
            <span class="size-2 rounded-full bg-slate-600" aria-hidden="true"></span>
            Not running.
          {/if}
        </p>
      </div>

      {#if stream.spotifyRunning}
        <button
          type="button"
          onclick={() => stream.stopSpotify()}
          class="rounded-md border border-[var(--color-border-subtle)] px-4 py-2 text-sm text-slate-300 transition-colors hover:bg-[var(--color-surface)]"
        >
          Stop capture
        </button>
      {:else}
        <button
          type="button"
          onclick={() => stream.startSpotify()}
          class="rounded-md bg-[var(--color-accent)] px-4 py-2 text-sm font-medium text-slate-900 transition-opacity hover:opacity-90"
        >
          Start Spotify capture
        </button>
      {/if}
    </div>

    {#if stream.spotifyRunning}
      {#if !stream.spotifyConnected}
        <p
          class="mt-4 rounded-md border border-[var(--color-border-subtle)] bg-[var(--color-surface)] px-3 py-2 text-sm text-slate-400"
        >
          Open Spotify on any device, tap the speaker/Connect icon, and choose
          <span class="font-medium text-slate-300">MusicSync</span>. (Premium required.)
        </p>
      {/if}

      <div class="mt-4 border-t border-[var(--color-border-subtle)] pt-4">
        <SpotifyNowPlaying {spotify} />
      </div>

      <!-- Master transport, proxied to the Connect session. -->
      <div class="mt-4 flex items-center justify-center gap-3">
        <button
          type="button"
          onclick={() => stream.transport("prev")}
          disabled={!canTransport}
          aria-label="Previous"
          class="rounded-md border border-[var(--color-border-subtle)] px-3 py-2 text-slate-300 transition-colors hover:bg-[var(--color-surface)] disabled:opacity-40"
        >
          <span aria-hidden="true">{"⏮"}</span>
        </button>
        <button
          type="button"
          onclick={() => stream.transport(playing ? "pause" : "play")}
          disabled={!canTransport}
          aria-label={playing ? "Pause" : "Play"}
          class="rounded-md bg-[var(--color-accent)] px-5 py-2 text-lg text-slate-900 transition-opacity hover:opacity-90 disabled:opacity-40"
        >
          <span aria-hidden="true">{playing ? "⏸" : "▶"}</span>
        </button>
        <button
          type="button"
          onclick={() => stream.transport("next")}
          disabled={!canTransport}
          aria-label="Next"
          class="rounded-md border border-[var(--color-border-subtle)] px-3 py-2 text-slate-300 transition-colors hover:bg-[var(--color-surface)] disabled:opacity-40"
        >
          <span aria-hidden="true">{"⏭"}</span>
        </button>
      </div>
    {/if}
  </section>

  <!-- --------------------------------------------------- Speaker group -- -->
  <section
    class="rounded-lg border border-[var(--color-border-subtle)] bg-[var(--color-surface-raised)] p-6"
  >
    <div class="flex flex-wrap items-start justify-between gap-3">
      <div>
        <h2 class="text-sm font-medium text-slate-200">Speakers</h2>
        <p class="mt-1 text-sm text-slate-500">
          {#if stream.streaming}
            Streaming to {stream.targets.length}
            {stream.targets.length === 1 ? "speaker" : "speakers"}.
          {:else}
            Pick the speakers to play to, then start.
          {/if}
        </p>
      </div>

      {#if stream.streaming}
        <button
          type="button"
          onclick={() => stream.stopStream()}
          class="rounded-md border border-red-500/40 bg-red-500/10 px-4 py-2 text-sm font-medium text-red-200 transition-colors hover:bg-red-500/20"
        >
          Stop
        </button>
      {:else}
        <button
          type="button"
          onclick={() => stream.startStream(selected)}
          disabled={selected.length === 0 || !stream.spotifyRunning}
          class="rounded-md bg-[var(--color-accent)] px-4 py-2 text-sm font-medium text-slate-900 transition-opacity hover:opacity-90 disabled:opacity-50"
          title={!stream.spotifyRunning ? "Start Spotify capture first" : undefined}
        >
          Play Everywhere{#if selected.length > 0} ({selected.length}){/if}
        </button>
      {/if}
    </div>

    {#if !stream.spotifyRunning && !stream.streaming}
      <p class="mt-3 text-xs text-slate-500">Start Spotify capture above before playing.</p>
    {/if}

    <div class="mt-4">
      {#if stream.streaming}
        <!-- Live per-room controls: volume + delay + sender health. -->
        <div class="flex flex-col gap-2">
          {#each stream.targets as room (room.ip)}
            <RoomRow {room} />
          {/each}
        </div>
      {:else}
        <StreamGroupPicker bind:selected disabled={stream.streaming} />
      {/if}
    </div>
  </section>
</div>
