import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";

export type UpdateState = "idle" | "available" | "downloading" | "ready" | "error";
export type CheckOutcome = "available" | "upToDate" | "failed";

/**
 * Auto-updater controller — a reactive singleton. The silent launch check
 * (`start()`, called from the root layout) feeds an update banner; `checkNow()`
 * backs a manual button in Settings and reports the outcome inline. Downloads
 * come from the R2 `latest.json` endpoint configured in tauri.conf.json.
 *
 * MusicSync is LAN-only, so a failed check is the normal offline case and must
 * never be surfaced on launch.
 */
class Updates {
  state = $state<UpdateState>("idle");
  version = $state("");
  progress = $state(0); // 0..100 while downloading
  downloadedBytes = $state(0);
  totalBytes = $state(0);
  errorMsg = $state("");
  dismissed = $state(false);
  checking = $state(false);

  visible = $derived(this.state !== "idle" && !this.dismissed);

  private updateRef: Update | null = null;

  /** Silent check on launch — never surfaces failures (offline is normal). */
  async start(): Promise<void> {
    try {
      await this.doCheck();
    } catch (e) {
      console.log("[updater] launch check failed:", e);
    }
  }

  /** Manual check from Settings; surfaces the outcome to the caller. */
  async checkNow(): Promise<CheckOutcome> {
    this.checking = true;
    try {
      return (await this.doCheck()) ? "available" : "upToDate";
    } catch (e) {
      console.error("[updater] manual check failed:", e);
      return "failed";
    } finally {
      this.checking = false;
    }
  }

  private async doCheck(): Promise<boolean> {
    const update = await check();
    if (!update) return false;
    this.updateRef = update;
    this.version = update.version;
    if (this.state === "idle" || this.state === "error") this.state = "available";
    this.dismissed = false;
    return true;
  }

  async downloadAndInstall(): Promise<void> {
    if (!this.updateRef) return;
    this.state = "downloading";
    this.progress = 0;
    this.downloadedBytes = 0;
    this.totalBytes = 0;
    try {
      await this.updateRef.downloadAndInstall((event) => {
        switch (event.event) {
          case "Started":
            this.totalBytes = event.data.contentLength ?? 0;
            break;
          case "Progress":
            this.downloadedBytes += event.data.chunkLength;
            if (this.totalBytes > 0) {
              this.progress = Math.min((this.downloadedBytes / this.totalBytes) * 100, 100);
            }
            break;
          case "Finished":
            this.progress = 100;
            break;
        }
      });
      this.state = "ready";
    } catch (e) {
      this.errorMsg = e instanceof Error ? e.message : String(e);
      this.state = "error";
      console.error("[updater] download/install failed:", e);
    }
  }

  async restart(): Promise<void> {
    await relaunch();
  }

  dismiss(): void {
    this.dismissed = true;
  }
}

export const updates = new Updates();
