/**
 * Transient command-failure notifications (R3, NFR-2). A tiny reactive queue —
 * the devices store pushes a toast when an optimistic playback command is
 * rejected (after it has rolled the UI back), and the root layout renders them.
 *
 * Failures are handled inline on the `invoke` rejection rather than through a
 * Rust `command-failed` event: the frontend already awaits every playback
 * command, so it knows exactly which action failed and can pair the rollback
 * with its toast without a second channel.
 */

export type ToastTone = "error" | "info";

export interface Toast {
  id: number;
  message: string;
  tone: ToastTone;
}

/** How long a toast stays up before auto-dismissing. */
const TTL_MS = 4000;

class Toasts {
  items = $state<Toast[]>([]);

  #seq = 0;
  #timers = new Map<number, ReturnType<typeof setTimeout>>();

  /** Queue a toast; returns its id. Auto-dismisses after {@link TTL_MS}. */
  push(message: string, tone: ToastTone = "error"): number {
    const id = ++this.#seq;
    this.items = [...this.items, { id, message, tone }];
    this.#timers.set(
      id,
      setTimeout(() => this.dismiss(id), TTL_MS)
    );
    return id;
  }

  /** Convenience for the common case. */
  error(message: string): number {
    return this.push(message, "error");
  }

  dismiss(id: number): void {
    const timer = this.#timers.get(id);
    if (timer) {
      clearTimeout(timer);
      this.#timers.delete(id);
    }
    this.items = this.items.filter((t) => t.id !== id);
  }
}

export const toasts = new Toasts();
