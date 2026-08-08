<script lang="ts">
  import { clock } from "$lib/format";
  import type { SpotifyState } from "$lib/types";

  // Now-playing card driven by the S3 `spotify-state` event: cover art, title,
  // artist, and a coarse position readout. Defensive against absent state — when
  // nothing is loaded it shows the "pick MusicSync" hint instead.

  let { spotify }: { spotify: SpotifyState | null } = $props();

  const track = $derived(spotify?.track ?? null);
  const playing = $derived(spotify?.play_state === "playing");
</script>

<div class="flex items-center gap-4">
  <div
    class="grid size-20 shrink-0 place-items-center overflow-hidden rounded-md border border-[var(--color-border-subtle)] bg-[var(--color-surface)]"
  >
    {#if track?.art_url}
      <img src={track.art_url} alt="" class="size-full object-cover" />
    {:else}
      <span class="text-2xl text-slate-600" aria-hidden="true">{"\u{1F3B5}"}</span>
    {/if}
  </div>

  <div class="min-w-0 flex-1">
    {#if track}
      <p class="truncate text-sm font-medium text-white">{track.title}</p>
      <p class="truncate text-sm text-slate-400">{track.artist}</p>
      {#if track.album}
        <p class="truncate text-xs text-slate-500">{track.album}</p>
      {/if}
      <p class="mt-1 flex items-center gap-2 text-xs text-slate-500">
        <span
          class="size-2 rounded-full {playing ? 'bg-emerald-400' : 'bg-slate-600'}"
          aria-hidden="true"
        ></span>
        <span class="font-mono tabular-nums">
          {clock(spotify?.position_ms ?? 0)}{#if track.duration_ms}
            / {clock(track.duration_ms)}{/if}
        </span>
      </p>
    {:else if spotify?.connected}
      <p class="text-sm text-slate-300">Connected — press play in Spotify.</p>
      <p class="mt-1 text-xs text-slate-500">Now-playing details appear once a track starts.</p>
    {:else}
      <p class="text-sm text-slate-300">Waiting for Spotify…</p>
      <p class="mt-1 text-xs text-slate-500">
        Open Spotify and select <span class="font-medium text-slate-300">MusicSync</span> from the
        device list.
      </p>
    {/if}
  </div>
</div>
