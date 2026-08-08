<script lang="ts">
  import { clock } from "$lib/format";
  import type { DeviceSnapshot } from "$lib/types";

  // Now-playing line (R3). Metadata comes from UPnP GetPositionInfo via the
  // poller (Luci TRACK_INFO doesn't answer on LP10 firmware). Renders nothing
  // unless there is a title/artist or a known position to show.

  let { device }: { device: DeviceSnapshot } = $props();

  const track = $derived(device.track);
  const title = $derived(track?.title ?? "");
  const artist = $derived(track?.artist ?? "");
  const heading = $derived([title, artist].filter(Boolean).join(" — "));

  const hasTiming = $derived(!!track && (track.durationMs !== null || track.positionMs !== null));
  const show = $derived(device.online && (!!heading || hasTiming));

  const progress = $derived.by(() => {
    if (!track?.durationMs || track.durationMs <= 0) return null;
    const pos = track.positionMs ?? 0;
    return Math.max(0, Math.min(1, pos / track.durationMs));
  });
</script>

{#if show}
  <div class="flex flex-col gap-1">
    <div class="flex items-center gap-2 text-xs">
      <span aria-hidden="true" class="text-[var(--color-accent)]">♪</span>
      <span class="truncate text-slate-200">{heading || "Playing"}</span>
      {#if device.sourceLabel && device.sourceLabel !== "Idle"}
        <span class="ml-auto shrink-0 text-[11px] text-slate-500">{device.sourceLabel}</span>
      {/if}
    </div>

    {#if track?.album}
      <p class="truncate text-[11px] text-slate-500">{track.album}</p>
    {/if}

    {#if hasTiming}
      <div class="flex items-center gap-2">
        <div class="h-1 flex-1 overflow-hidden rounded-full bg-[var(--color-surface)]">
          {#if progress !== null}
            <div
              class="h-full rounded-full bg-[var(--color-accent)]/70 transition-[width] duration-500"
              style="width: {Math.round(progress * 100)}%"
            ></div>
          {/if}
        </div>
        <span class="shrink-0 font-mono text-[10px] text-slate-500 tabular-nums">
          {clock(track?.positionMs ?? 0)}{#if track?.durationMs} / {clock(track.durationMs)}{/if}
        </span>
      </div>
    {/if}
  </div>
{/if}
