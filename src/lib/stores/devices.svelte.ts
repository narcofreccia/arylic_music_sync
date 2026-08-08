import type { UnlistenFn } from "@tauri-apps/api/event";
import { commands } from "$lib/tauri/commands";
import { onDeviceOffline, onDeviceUpdated } from "$lib/tauri/events";
import type { DeviceDetail, DeviceSnapshot } from "$lib/types";

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
      for (const device of devices) next[device.uuid] = device;
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

  /** Reassignment (not mutation) is what makes the `$derived` lists recompute. */
  #apply(snapshot: DeviceSnapshot): void {
    this.map = { ...this.map, [snapshot.uuid]: snapshot };
  }
}

export const devices = new Devices();
