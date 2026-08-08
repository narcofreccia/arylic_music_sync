import type { UnlistenFn } from "@tauri-apps/api/event";
import { commands, errorMessage } from "$lib/tauri/commands";
import { onSpotifyState, onStreamState } from "$lib/tauri/events";
import { toasts } from "$lib/stores/toasts.svelte";
import { devices } from "$lib/stores/devices.svelte";
import type {
  PlayerCmd,
  SpotifyState,
  StreamDeviceStatus,
  StreamStatus,
  StreamTarget,
} from "$lib/types";

/** AirPlay-1 RAOP control port the LP10 listens on (design doc §4). */
const RAOP_PORT = 5000;

/** Optimistic tuning for the per-room sliders (mirrors the devices store). */
const VOLUME_DEBOUNCE_MS = 80;
const VOLUME_GRACE_MS = 1200;
const DELAY_DEBOUNCE_MS = 120;
const DELAY_GRACE_MS = 1400;

/** An in-flight optimistic override for one streaming receiver, keyed by IP. */
interface Pending {
  /** Software gain, 0.0..=1.0. */
  volume?: number;
  /** Software delay, ms. */
  delayMs?: number;
  /** Unix ms after which the override is abandoned. */
  expiresAt: number;
}

/** The idle streaming status — what the UI shows before anything starts. */
function idleStatus(): StreamStatus {
  return {
    active: false,
    source: null,
    anchor_ntp: null,
    latency_frames: 0,
    devices: [],
  };
}

/**
 * "Play Everywhere" state — a reactive singleton mirroring the S3 Spotify
 * capture (`spotify-state`) and the S2 RAOP fan-out (`stream-state`). Rust is
 * the source of truth: this store caches the events and applies short-lived
 * optimistic overrides to the per-room sliders so a slower event can't snap them
 * back (the same pattern as the devices store).
 *
 * `start()` is idempotent and called once from the root layout, so listeners are
 * registered exactly once for the window's lifetime; pages read the store.
 */
class Stream {
  /** Latest `spotify-state`; null until the first status/event lands. */
  spotify = $state<SpotifyState | null>(null);
  /** Latest `stream-state` (idle when nothing is streaming). */
  status = $state<StreamStatus>(idleStatus());

  /** True once listeners are registered and the initial status hydrated. */
  loading = $state(true);

  /** Whether the RAOP fan-out is live. */
  streaming = $derived(this.status.active);
  /** Per-receiver rows the UI renders (with any optimistic override applied). */
  targets = $derived<StreamDeviceStatus[]>(this.status.devices);

  /** The advertised endpoint is up and a Spotify client is attached. */
  spotifyRunning = $derived(this.spotify?.running ?? false);
  spotifyConnected = $derived(this.spotify?.connected ?? false);

  #started = false;
  #unlisten: UnlistenFn[] = [];

  /** Live optimistic overrides, keyed by receiver IP. */
  #pending: Record<string, Pending> = {};
  /** Debounce timers + monotonic seqs per IP, so only the latest write sends. */
  #volTimer: Record<string, ReturnType<typeof setTimeout>> = {};
  #volSeq: Record<string, number> = {};
  #delayTimer: Record<string, ReturnType<typeof setTimeout>> = {};
  #delaySeq: Record<string, number> = {};

  /** Idempotent: safe to call from an effect that re-runs. */
  async start(): Promise<void> {
    if (this.#started) return;
    this.#started = true;

    // Listen before hydrating, so an event arriving mid-round-trip is applied on
    // top of the status rather than lost between the two.
    try {
      this.#unlisten = await Promise.all([
        onSpotifyState((state) => (this.spotify = state)),
        onStreamState((status) => this.#applyStatus(status)),
      ]);
    } catch (e) {
      console.error("[stream] could not subscribe to streaming events:", e);
    }

    await this.hydrate();
  }

  /** Drop the subscriptions (window teardown). */
  stop(): void {
    for (const off of this.#unlisten) off();
    this.#unlisten = [];
    this.#started = false;
  }

  /** Pull the current Spotify + streaming status once at boot. */
  async hydrate(): Promise<void> {
    try {
      const [spotify, status] = await Promise.all([
        commands.spotifyStatus(),
        commands.streamStatus(),
      ]);
      this.spotify = spotify;
      this.#applyStatus(status);
    } catch (e) {
      console.error("[stream] status hydration failed:", e);
    } finally {
      this.loading = false;
    }
  }

  // ------------------------------------------------------------ spotify (S3) --

  /** Start advertising the "MusicSync" Connect endpoint. */
  async startSpotify(): Promise<void> {
    try {
      this.spotify = await commands.spotifyStart();
    } catch (e) {
      toasts.error(errorMessage(e, "Could not start Spotify capture."));
    }
  }

  /** Stop advertising and tear the capture down. */
  async stopSpotify(): Promise<void> {
    try {
      this.spotify = await commands.spotifyStop();
    } catch (e) {
      toasts.error(errorMessage(e, "Could not stop Spotify capture."));
    }
  }

  /** Master transport, proxied to the connected Spotify session. */
  async transport(cmd: PlayerCmd): Promise<void> {
    try {
      switch (cmd) {
        case "play":
          await commands.spotifyPlay();
          break;
        case "pause":
        case "stop":
          await commands.spotifyPause();
          break;
        case "next":
          await commands.spotifyNext();
          break;
        case "prev":
          await commands.spotifyPrev();
          break;
      }
    } catch (e) {
      toasts.error(errorMessage(e, "Spotify didn't accept that."));
    }
  }

  // ---------------------------------------------------------- streaming (S2) --

  /**
   * Start streaming Spotify to the selected devices in sync. `targetUuids` are
   * device UUIDs from the devices store; each resolves to its current IP.
   */
  async startStream(targetUuids: string[]): Promise<void> {
    const targets: StreamTarget[] = [];
    for (const uuid of targetUuids) {
      const device = devices.get(uuid);
      if (!device) continue;
      targets.push({
        uuid,
        name: device.displayName,
        ip: device.ip,
        raop_port: RAOP_PORT,
      });
    }
    if (targets.length === 0) {
      toasts.error("Pick at least one speaker to play to.");
      return;
    }
    try {
      this.#applyStatus(await commands.streamStart(targets, { kind: "spotify" }));
    } catch (e) {
      toasts.error(errorMessage(e, "Could not start the stream."));
    }
  }

  /** Stop the RAOP fan-out (keeps Spotify capture running). */
  async stopStream(): Promise<void> {
    try {
      this.#applyStatus(await commands.streamStop());
    } catch (e) {
      toasts.error(errorMessage(e, "Could not stop the stream."));
    }
  }

  /**
   * Optimistic per-room volume (`0.0..=1.0`): the slider moves now, the write is
   * debounced, and the override survives a grace window so an in-flight
   * `stream-state` can't snap it back. A rejected write rolls back and toasts.
   */
  setDeviceVolume(ip: string, vol: number): void {
    const clamped = Math.max(0, Math.min(1, vol));
    this.#overlay(ip, { volume: clamped }, VOLUME_GRACE_MS);

    clearTimeout(this.#volTimer[ip]);
    const seq = (this.#volSeq[ip] = (this.#volSeq[ip] ?? 0) + 1);
    this.#volTimer[ip] = setTimeout(async () => {
      try {
        this.#applyStatus(await commands.streamSetDeviceVolume(ip, clamped));
        this.#extend(ip, VOLUME_GRACE_MS);
      } catch (e) {
        if (seq === this.#volSeq[ip]) {
          this.#rollback(ip, "volume");
          toasts.error(errorMessage(e, "Could not set the volume."));
        }
      }
    }, VOLUME_DEBOUNCE_MS);
  }

  /** Optimistic per-room delay in ms (trims room-to-room skew). */
  setDeviceDelay(ip: string, ms: number): void {
    const clamped = Math.max(0, Math.round(ms));
    this.#overlay(ip, { delayMs: clamped }, DELAY_GRACE_MS);

    clearTimeout(this.#delayTimer[ip]);
    const seq = (this.#delaySeq[ip] = (this.#delaySeq[ip] ?? 0) + 1);
    this.#delayTimer[ip] = setTimeout(async () => {
      try {
        this.#applyStatus(await commands.streamSetDeviceDelay(ip, clamped));
        this.#extend(ip, DELAY_GRACE_MS);
      } catch (e) {
        if (seq === this.#delaySeq[ip]) {
          this.#rollback(ip, "delayMs");
          toasts.error(errorMessage(e, "Could not set the delay."));
        }
      }
    }, DELAY_DEBOUNCE_MS);
  }

  // ----------------------------------------------------- override plumbing --

  /** Apply an optimistic patch to the visible rows and remember it as pending. */
  #overlay(ip: string, patch: Partial<Pending>, graceMs: number): void {
    const prev = this.#pending[ip];
    this.#pending[ip] = { ...prev, ...patch, expiresAt: Date.now() + graceMs };
    this.status = { ...this.status, devices: this.#merge(this.status.devices) };
  }

  /** Push a grace window out (called after a successful send). */
  #extend(ip: string, graceMs: number): void {
    const p = this.#pending[ip];
    if (p) p.expiresAt = Date.now() + graceMs;
  }

  /** Drop one overridden field so the server value takes over on the next merge. */
  #rollback(ip: string, field: keyof Omit<Pending, "expiresAt">): void {
    const p = this.#pending[ip];
    if (p) {
      delete p[field];
      if (Object.keys(p).length <= 1) delete this.#pending[ip];
    }
    this.status = { ...this.status, devices: this.#merge(this.status.devices) };
  }

  /** Merge a fresh device list with any live overrides (pending wins until grace lapses/confirms). */
  #merge(list: StreamDeviceStatus[]): StreamDeviceStatus[] {
    return list.map((d) => {
      const p = this.#pending[d.ip];
      if (!p) return d;
      if (Date.now() >= p.expiresAt) {
        delete this.#pending[d.ip];
        return d;
      }
      const merged = { ...d };
      if (p.volume !== undefined) {
        if (d.volume === p.volume) delete p.volume;
        else merged.volume = p.volume;
      }
      if (p.delayMs !== undefined) {
        if (d.delay_ms === p.delayMs) delete p.delayMs;
        else merged.delay_ms = p.delayMs;
      }
      if (p.volume === undefined && p.delayMs === undefined) delete this.#pending[d.ip];
      return merged;
    });
  }

  /** Store the server truth, apply overrides, and reassign to recompute. */
  #applyStatus(status: StreamStatus): void {
    this.status = { ...status, devices: this.#merge(status.devices) };
  }
}

export const stream = new Stream();
