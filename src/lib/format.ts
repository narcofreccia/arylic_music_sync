/** Small display helpers shared by the device components. */

/** `m:ss` from a Linkplay position/length in ms. Empty for unknown lengths. */
export function clock(ms: number): string {
  if (!Number.isFinite(ms) || ms <= 0) return "0:00";
  const total = Math.floor(ms / 1000);
  const minutes = Math.floor(total / 60);
  const seconds = total % 60;
  return `${minutes}:${seconds.toString().padStart(2, "0")}`;
}

/**
 * Coarse "when was this device last around" for an offline card. Coarse on
 * purpose: a poller with a 3 s cycle can't honestly claim second precision
 * after the fact, and "3 minutes ago" is what the user actually needs.
 */
export function sinceLabel(unixMs: number | null): string {
  if (!unixMs) return "never seen";
  const seconds = Math.max(0, Math.round((Date.now() - unixMs) / 1000));
  if (seconds < 60) return "seen just now";
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `seen ${minutes} min ago`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return `seen ${hours} h ago`;
  return `seen ${Math.round(hours / 24)} d ago`;
}
