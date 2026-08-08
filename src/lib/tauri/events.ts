import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  DeviceCandidate,
  DeviceSnapshot,
  ScanComplete,
  ScanProgress,
} from "$lib/types";

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

/**
 * Discovery progress (FR-4), throttled to ≤10 Hz on the Rust side. `phase` says
 * which strategy the counters belong to — the three run concurrently, so events
 * from different phases interleave.
 */
export function onScanProgress(
  handler: (progress: ScanProgress) => void
): Promise<UnlistenFn> {
  return listen<ScanProgress>("scan-progress", (event) => handler(event.payload));
}

/**
 * A confirmed candidate (a DDMS banner or a Luci `DevInfo`), emitted live rather
 * than batched at the end — the user should see results filling in as they land.
 */
export function onScanDeviceFound(
  handler: (candidate: DeviceCandidate) => void
): Promise<UnlistenFn> {
  return listen<{ candidate: DeviceCandidate }>("scan-device-found", (event) =>
    handler(event.payload.candidate)
  );
}

/** The scan ended. `cancelled` distinguishes "nothing found" from "stopped". */
export function onScanComplete(
  handler: (result: ScanComplete) => void
): Promise<UnlistenFn> {
  return listen<ScanComplete>("scan-complete", (event) => handler(event.payload));
}
