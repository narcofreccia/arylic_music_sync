import type { UnlistenFn } from "@tauri-apps/api/event";
import { commands, errorMessage } from "$lib/tauri/commands";
import { onDeviceOffline, onDeviceUpdated } from "$lib/tauri/events";
import { toasts } from "$lib/stores/toasts.svelte";
import type { DeviceSnapshot, DeviceDetail, PlayerCmd } from "$lib/types";

/**
 * An in-flight optimistic override for one device. While it lives, its fields
 * win over whatever the poller reports, so a slower poll cycle can't snap the
 * slider back to a stale value (NFR-2). It clears when the server confirms the
 * value, when the grace window lapses, or on a command failure (rollback).
 */
interface Pending {
  volume?: number;
  mute?: boolean;
  playState?: number;
  /** Unix ms after which the override is abandoned. */
  expiresAt: number;
}

/** Optimistic tuning (NFR-2). */
const VOLUME_DEBOUNCE_MS = 80;
const VOLUME_GRACE_MS = 1200;
const MUTE_GRACE_MS = 700;
const TRANSPORT_GRACE_MS = 900;

/**
 * Live device state (FR-6/FR-18/FR-19) — a reactive singleton mirroring the
 * Rust poller. Rust is the source of truth: this store never invents state, it
 * only caches what `list_devices` and the `device-updated` events say.
 *
 * `start()` is called once from the root layout after auth clears, so listeners
 * are registered exactly once for the lifetime of the window; pages read the
 * store. No optimistic updates yet — volume/transport (and their rollback) land
 * with M5.
 */
class Devices {
  /** Keyed by UUID, which is the identity the Rust side uses everywhere. */
  map = $state<Record<string, DeviceSnapshot>>({});
  /** True until the first `list_devices` resolves. */
  loading = $state(true);
  /** Last hydration failure — device errors are surfaced per action, not here. */
  error = $state("");

  /** Stable ordering: online first, then by display name (a moving list is unusable). */
  list = $derived(
    Object.values(this.map).sort((a, b) => {
      if (a.online !== b.online) return a.online ? -1 : 1;
      return a.displayName.localeCompare(b.displayName);
    })
  );

  online = $derived(this.list.filter((d) => d.online));
  masters = $derived(this.list.filter((d) => d.role === "master"));
  /** Everything currently in a group, master or slave (topology input for R2). */
  grouped = $derived(this.list.filter((d) => d.role !== "solo"));
  count = $derived(this.list.length);

  #started = false;
  #unlisten: UnlistenFn[] = [];

  /** Live optimistic overrides, keyed by uuid (NFR-2). */
  #pending: Record<string, Pending> = {};
  /** Last snapshot the server actually sent, per uuid — the rollback target. */
  #server: Record<string, DeviceSnapshot> = {};
  /** Debounce timer + monotonic seq per uuid, so only the latest volume sends. */
  #volTimer: Record<string, ReturnType<typeof setTimeout>> = {};
  #volSeq: Record<string, number> = {};

  /** Idempotent: safe to call from an effect that re-runs. */
  async start(): Promise<void> {
    if (this.#started) return;
    this.#started = true;

    // Listen before hydrating, so an event arriving mid-round-trip is applied
    // on top of the list rather than lost between the two.
    try {
      this.#unlisten = await Promise.all([
        onDeviceUpdated((snapshot) => this.#apply(snapshot)),
        onDeviceOffline((snapshot) => this.#apply(snapshot)),
      ]);
    } catch (e) {
      console.error("[devices] could not subscribe to device events:", e);
    }

    await this.hydrate();
  }

  /** Drop the subscriptions (window teardown). */
  stop(): void {
    for (const off of this.#unlisten) off();
    this.#unlisten = [];
    this.#started = false;
  }

  /** Re-read the persisted list plus its latest known state. */
  async hydrate(): Promise<void> {
    try {
      const devices = await commands.listDevices();
      const next: Record<string, DeviceSnapshot> = {};
      for (const device of devices) {
        this.#server[device.uuid] = device;
        next[device.uuid] = this.#merge(device);
      }
      this.map = next;
      this.error = "";
    } catch (e) {
      console.error("[devices] list_devices failed:", e);
      this.error = "Could not read the saved device list.";
    } finally {
      this.loading = false;
    }
  }

  get(uuid: string): DeviceSnapshot | undefined {
    return this.map[uuid];
  }

  /** FR-5. Throws the `{ code, message }` envelope on a bad IP or no answer. */
  async add(ip: string): Promise<DeviceSnapshot> {
    const snapshot = await commands.addDevice(ip);
    this.#apply(snapshot);
    return snapshot;
  }

  /** FR-8. */
  async remove(uuid: string): Promise<void> {
    await commands.removeDevice(uuid);
    clearTimeout(this.#volTimer[uuid]);
    delete this.#volTimer[uuid];
    delete this.#pending[uuid];
    delete this.#server[uuid];
    const { [uuid]: _removed, ...rest } = this.map;
    this.map = rest;
  }

  /** FR-7. `alias: null` restores the device's own name. */
  async rename(uuid: string, alias: string | null, pushToDevice = false): Promise<void> {
    this.#apply(await commands.renameDevice(uuid, alias, pushToDevice));
  }

  /** FR-9: a live round trip for the detail pane. */
  detail(uuid: string): Promise<DeviceDetail> {
    return commands.getStatus(uuid);
  }

  /** Wake the poll loop; the answer arrives as a `device-updated` event. */
  async refresh(uuid: string): Promise<void> {
    await commands.refreshDevice(uuid);
  }

  // ------------------------------------------------------- playback (R3) --

  /**
   * Optimistic volume (NFR-2): the slider moves now, the write is debounced
   * ~80 ms, and the override survives ~1.2 s so an in-flight poll can't snap it
   * back. On a rejected write we roll the slider back and toast.
   */
  setVolume(uuid: string, vol: number): void {
    const clamped = Math.max(0, Math.min(100, Math.round(vol)));
    this.#overlay(uuid, { volume: clamped }, VOLUME_GRACE_MS);

    clearTimeout(this.#volTimer[uuid]);
    const seq = (this.#volSeq[uuid] = (this.#volSeq[uuid] ?? 0) + 1);
    this.#volTimer[uuid] = setTimeout(async () => {
      try {
        await commands.setVolume(uuid, clamped);
        // Refresh the grace window from the send instant so the confirming poll
        // has time to arrive before the override lapses.
        this.#extend(uuid, VOLUME_GRACE_MS);
      } catch (e) {
        if (seq === this.#volSeq[uuid]) {
          this.#rollback(uuid, "volume");
          toasts.error(errorMessage(e, "Could not set the volume."));
        }
      }
    }, VOLUME_DEBOUNCE_MS);
  }

  /** Optimistic mute toggle, shorter grace than volume. */
  async setMute(uuid: string, mute: boolean): Promise<void> {
    this.#overlay(uuid, { mute }, MUTE_GRACE_MS);
    try {
      await commands.setMute(uuid, mute);
    } catch (e) {
      this.#rollback(uuid, "mute");
      toasts.error(errorMessage(e, "Could not change mute."));
    }
  }

  /** Transport, optimistic for play/pause/stop (predictable play-state). */
  async player(uuid: string, cmd: PlayerCmd): Promise<void> {
    const optimistic =
      cmd === "play" ? 1 : cmd === "pause" || cmd === "stop" ? 0 : undefined;
    if (optimistic !== undefined) {
      this.#overlay(uuid, { playState: optimistic }, TRANSPORT_GRACE_MS);
    }
    try {
      await commands.playerCmd(uuid, cmd);
    } catch (e) {
      if (optimistic !== undefined) this.#rollback(uuid, "playState");
      toasts.error(errorMessage(e, "The device didn't accept that."));
    }
  }

  // ----------------------------------------------------- override plumbing --

  /** Apply an optimistic patch to the display and remember it as pending. */
  #overlay(uuid: string, patch: Partial<Pending>, graceMs: number): void {
    const prev = this.#pending[uuid];
    this.#pending[uuid] = {
      ...prev,
      ...patch,
      expiresAt: Date.now() + graceMs,
    };
    const base = this.map[uuid];
    if (base) this.map = { ...this.map, [uuid]: { ...base, ...patch } };
  }

  /** Push the grace window out (called after a successful send). */
  #extend(uuid: string, graceMs: number): void {
    const p = this.#pending[uuid];
    if (p) p.expiresAt = Date.now() + graceMs;
  }

  /** Drop one overridden field and restore the server's value for it. */
  #rollback(uuid: string, field: keyof Omit<Pending, "expiresAt">): void {
    const p = this.#pending[uuid];
    if (p) {
      delete p[field];
      if (Object.keys(p).length <= 1) delete this.#pending[uuid];
    }
    const server = this.#server[uuid];
    const base = this.map[uuid];
    if (server && base) {
      this.map = { ...this.map, [uuid]: { ...base, [field]: server[field] } };
    }
  }

  /**
   * Merge a fresh server snapshot with any live override. A pending field wins
   * until its grace lapses or the server confirms it; a confirmed field is
   * cleared so the store stops shadowing the truth.
   */
  #merge(snapshot: DeviceSnapshot): DeviceSnapshot {
    const p = this.#pending[snapshot.uuid];
    if (!p) return snapshot;
    if (Date.now() >= p.expiresAt) {
      delete this.#pending[snapshot.uuid];
      return snapshot;
    }
    const merged = { ...snapshot };
    // A device going offline drops every override — nothing to shadow anymore.
    if (!snapshot.online) {
      delete this.#pending[snapshot.uuid];
      return snapshot;
    }
    if (p.volume !== undefined) {
      if (snapshot.volume === p.volume) delete p.volume;
      else merged.volume = p.volume;
    }
    if (p.mute !== undefined) {
      if (snapshot.mute === p.mute) delete p.mute;
      else merged.mute = p.mute;
    }
    if (p.playState !== undefined) {
      if (snapshot.playState === p.playState) delete p.playState;
      else merged.playState = p.playState;
    }
    // Nothing left to override → forget the entry.
    if (p.volume === undefined && p.mute === undefined && p.playState === undefined) {
      delete this.#pending[snapshot.uuid];
    }
    return merged;
  }

  /** Record the server truth, merge overrides, and reassign to recompute. */
  #apply(snapshot: DeviceSnapshot): void {
    this.#server[snapshot.uuid] = snapshot;
    this.map = { ...this.map, [snapshot.uuid]: this.#merge(snapshot) };
  }
}

export const devices = new Devices();
