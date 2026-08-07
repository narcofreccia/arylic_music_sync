import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { DeviceSnapshot } from "$lib/types";

/**
 * Typed wrappers over the Rust event stream (brief.md §7). Same rule as
 * `commands.ts`: the event-name strings live here only, so a rename on the Rust
 * side is a compile error rather than a listener that silently never fires.
 *
 * Registration belongs to the devices store (called once from the root layout);
 * components read the store instead of subscribing themselves — otherwise every
 * navigation would add another listener.
 */

/** Emitted whenever a device's snapshot actually changed (including going offline). */
export function onDeviceUpdated(
  handler: (snapshot: DeviceSnapshot) => void
): Promise<UnlistenFn> {
  return listen<DeviceSnapshot>("device-updated", (event) => handler(event.payload));
}

/**
 * Emitted once when a device crosses the offline threshold (3 failed cycles) —
 * the edge, not every failed poll. `device-updated` carries the same snapshot;
 * this one exists for notifications.
 */
export function onDeviceOffline(
  handler: (snapshot: DeviceSnapshot) => void
): Promise<UnlistenFn> {
  return listen<DeviceSnapshot>("device-offline", (event) => handler(event.payload));
}
