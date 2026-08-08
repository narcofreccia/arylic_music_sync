import type { UnlistenFn } from "@tauri-apps/api/event";
import { commands, errorMessage } from "$lib/tauri/commands";
import { onScanComplete, onScanDeviceFound, onScanProgress } from "$lib/tauri/events";
import type { DeviceCandidate, ScanOptions, ScanPhase } from "$lib/types";

/** Stable identity of a candidate: uuid, then usn, then ip — mirrors Rust. */
export function candidateKey(c: DeviceCandidate): string {
  if (c.uuid.trim()) return `uuid:${c.uuid.trim()}`;
  if (c.usn.trim()) return `usn:${c.usn.trim().toLowerCase()}`;
  return `ip:${c.ip}`;
}

/**
 * Network discovery state (FR-4) — a reactive singleton mirroring the Rust
 * scanner. Same contract as the devices store: Rust owns the truth, this only
 * caches what the events say, and `start()` is called once from the root layout
 * so listeners never stack per navigation.
 *
 * Only one scan runs at a time; asking for another cancels and restarts the
 * first (the Rust `scan` command's documented policy). Because the superseded
 * run still resolves — and still emits its own `scan-complete` — every result
 * is stamped with the run it belongs to, and stale ones are dropped.
 */
class Scan {
  running = $state(false);
  /** The strategy the last progress event came from; the three interleave. */
  phase = $state<ScanPhase | null>(null);
  scanned = $state(0);
  /** 0 while the running phase has no denominator (mDNS/SSDP). */
  total = $state(0);
  found = $state<DeviceCandidate[]>([]);
  /** True when the last scan was stopped rather than allowed to finish. */
  cancelled = $state(false);
  error = $state("");

  /** UUIDs the user added from this scan's results, for instant row feedback. */
  added = $state<Record<string, true>>({});

  /** Fraction complete for the current phase, or null when indeterminate. */
  progress = $derived(this.total > 0 ? Math.min(this.scanned / this.total, 1) : null);

  /** Candidates not already in the device list — what "Add all new" acts on. */
  newCandidates = $derived(
    this.found.filter((c) => !c.alreadyAdded && !this.added[candidateKey(c)])
  );

  #started = false;
  #unlisten: UnlistenFn[] = [];
  /**
   * Which run events belong to. Bumped on every `scan()`, so a cancelled run's
   * trailing events can't clobber the one that replaced it.
   */
  #run = 0;

  /** Idempotent: safe to call from an effect that re-runs. */
  async start(): Promise<void> {
    if (this.#started) return;
    this.#started = true;

    try {
      this.#unlisten = await Promise.all([
        onScanProgress((progress) => {
          if (!this.running) return;
          this.phase = progress.phase;
          this.scanned = progress.scanned;
          this.total = progress.total;
        }),
        onScanDeviceFound((candidate) => {
          if (!this.running) return;
          this.#merge(candidate);
        }),
        onScanComplete((result) => {
          if (!this.running) return;
          this.cancelled = result.cancelled;
        }),
      ]);
    } catch (e) {
      console.error("[scan] could not subscribe to scan events:", e);
    }
  }

  /** Drop the subscriptions (window teardown). */
  stop(): void {
    for (const off of this.#unlisten) off();
    this.#unlisten = [];
    this.#started = false;
  }

  /**
   * FR-4. Resolves when the scan ends; results also stream in via events, so
   * callers usually just await it for the error.
   */
  async scan(options: ScanOptions): Promise<void> {
    const run = ++this.#run;
    this.running = true;
    this.cancelled = false;
    this.error = "";
    this.phase = null;
    this.scanned = 0;
    this.total = 0;
    this.found = [];
    this.added = {};

    try {
      const candidates = await commands.scan(options);
      if (run !== this.#run) return; // Superseded by a newer scan.
      // The events already delivered these; reconciling with the command's
      // return value covers anything confirmed between the last event and the
      // end of the run.
      for (const candidate of candidates) this.#merge(candidate);
    } catch (e) {
      if (run !== this.#run) return;
      this.error = errorMessage(e, "The scan could not be started.");
    } finally {
      if (run === this.#run) {
        this.running = false;
        this.phase = null;
      }
    }
  }

  /** Stop the running scan; the pending `scan()` resolves with what it found. */
  async cancel(): Promise<void> {
    try {
      await commands.cancelScan();
    } catch (e) {
      console.error("[scan] cancel_scan failed:", e);
    }
  }

  /** Mark a candidate as added, so its row stops offering the button. */
  markAdded(candidate: DeviceCandidate): void {
    this.added = { ...this.added, [candidateKey(candidate)]: true };
  }

  /** Clear the results list without touching the saved devices. */
  clear(): void {
    this.found = [];
    this.added = {};
    this.cancelled = false;
    this.error = "";
  }

  /** Insert or refresh by identity — the same key the Rust side dedupes on. */
  #merge(candidate: DeviceCandidate): void {
    const key = candidateKey(candidate);
    const at = this.found.findIndex((c) => candidateKey(c) === key);
    if (at === -1) {
      this.found = [...this.found, candidate];
      return;
    }
    const next = [...this.found];
    next[at] = candidate;
    this.found = next;
  }
}

export const scan = new Scan();
