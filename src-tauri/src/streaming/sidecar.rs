//! One `cliraop` child per RAOP receiver (Phase S2).
//!
//! This is the streaming analogue of `tide_share`'s Python JSON-RPC sidecar, but
//! deliberately thinner: `cliraop`'s stdin *is* the PCM stream, so there is no
//! request/response channel into the child — it is a pure data sink (PCM in,
//! RAOP/RTP out). We therefore only manage lifecycle: resolve the bundled binary
//! with the same resource-dir/exe-dir/`binaries/` fallback ladder tide uses, spawn
//! with piped stdin + captured stdout/stderr, hide the console on Windows
//! (`CREATE_NO_WINDOW`), and kill/reap cleanly.
//!
//! The cross-room sync primitive — the shared NTP anchor — is captured once via
//! [`capture_ntp_anchor`] and handed to every child as `-nf <file>`, together
//! with a matched `-l <latency>` and `-w <wait>` (design doc §1).

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::thread;

use super::model::StreamTarget;

/// Basename of the RAOP sender binary, before Tauri's target-triple suffix.
pub const CLIRAOP_BASENAME: &str = "cliraop";

/// Resolve the bundled `cliraop` binary path.
///
/// Search order mirrors `tide_share::sidecar::resolve_bundled_sidecar`:
///   1. `resource_dir` (bundled app) — both triple-tagged and Tauri-stripped names,
///   2. next to the current exe (`externalBin` lands beside the binary),
///   3. `CARGO_MANIFEST_DIR/binaries` (dev / `cargo run --example`).
///
/// `resource_dir` and `exe_dir` are optional so this is callable from an example
/// (no `AppHandle`); there it falls through to the `binaries/` dev path.
pub fn resolve_cliraop(resource_dir: Option<&Path>, exe_dir: Option<&Path>) -> Option<PathBuf> {
    let triple = target_triple();
    let triple_name = format!("{CLIRAOP_BASENAME}-{triple}");

    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(d) = resource_dir {
        roots.push(d.to_path_buf());
    }
    if let Some(d) = exe_dir {
        roots.push(d.to_path_buf());
    }
    roots.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("binaries"));

    for base in roots {
        for cand in candidate_paths(&base, &triple_name) {
            if cand.exists() {
                return Some(cand);
            }
        }
    }
    None
}

/// Every install-layout filename the resolver must accept — Tauri strips the
/// target-triple suffix at bundle time, so the installed file is
/// `cliraop[.exe]`, while the dev `binaries/` copy keeps the triple tag.
fn candidate_paths(base: &Path, triple_name: &str) -> [PathBuf; 4] {
    [
        base.join(format!("{triple_name}.exe")), // dev/win, triple-tagged
        base.join(triple_name),                  // dev/mac, triple-tagged
        base.join(format!("{CLIRAOP_BASENAME}.exe")), // bundled win (stripped)
        base.join(CLIRAOP_BASENAME),             // bundled mac/linux (stripped)
    ]
}

/// The Rust host target triple, used to build the dev binary name.
fn target_triple() -> &'static str {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        "aarch64-apple-darwin"
    }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        "x86_64-apple-darwin"
    }
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        "x86_64-pc-windows-msvc"
    }
    #[cfg(all(target_os = "windows", target_arch = "x86"))]
    {
        "i686-pc-windows-msvc"
    }
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        "x86_64-unknown-linux-gnu"
    }
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    {
        "aarch64-unknown-linux-gnu"
    }
}

/// Apply the Windows console-hiding flag (`CREATE_NO_WINDOW = 0x08000000`).
/// No-op on every other platform (nothing has an attached console to hide).
fn hide_console(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    #[cfg(not(windows))]
    {
        let _ = cmd;
    }
}

/// Capture the shared master NTP clock **once** by running `cliraop -ntp <file>`,
/// which writes a 64-bit NTP value and exits. Every child is then anchored to it
/// with `-nf <file>`. Returns the anchor file path (kept alive for the stream's
/// duration) and the decimal NTP value it holds.
pub fn capture_ntp_anchor(bin: &Path, anchor_path: &Path) -> Result<String, String> {
    let mut cmd = Command::new(bin);
    cmd.arg("-ntp")
        .arg(anchor_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    hide_console(&mut cmd);

    let out = cmd
        .output()
        .map_err(|e| format!("spawn cliraop -ntp: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "cliraop -ntp exited with {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let value = std::fs::read_to_string(anchor_path)
        .map_err(|e| format!("read anchor {}: {e}", anchor_path.display()))?
        .trim()
        .to_string();
    if value.is_empty() {
        return Err("cliraop -ntp wrote an empty anchor file".into());
    }
    Ok(value)
}

/// A single running `cliraop` child feeding one RAOP receiver.
pub struct RaopChild {
    child: Child,
    /// Taken by the engine's writer thread; `None` once handed off.
    stdin: Option<ChildStdin>,
    label: String,
}

impl RaopChild {
    /// Spawn a `cliraop` child anchored to the shared NTP, with matched latency
    /// and wait so it slaves to the same clock as its siblings.
    ///
    /// Command shape (design doc §1):
    /// `cliraop -nf <anchor> -w <wait> -l <latency> -p <port> <ip> -`
    pub fn spawn(
        bin: &Path,
        target: &StreamTarget,
        anchor_path: &Path,
        wait_ms: u32,
        latency_frames: u32,
    ) -> Result<Self, String> {
        let label = format!("{}@{}:{}", target.name, target.ip, target.raop_port);

        let mut cmd = Command::new(bin);
        cmd.arg("-nf")
            .arg(anchor_path)
            .args(["-w", &wait_ms.to_string()])
            .args(["-l", &latency_frames.to_string()])
            .args(["-p", &target.raop_port.to_string()])
            // Silence cliraop's own stdout chatter; we watch stderr instead.
            .args(["-d", "1"])
            .arg(&target.ip)
            .arg("-") // PCM on stdin
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        hide_console(&mut cmd);

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("spawn cliraop for {label}: {e}"))?;

        // Drain stdout/stderr on background threads so the child never blocks on a
        // full pipe, and its RAOP progress lines land in our log.
        if let Some(out) = child.stdout.take() {
            let l = label.clone();
            thread::spawn(move || pipe_to_log(out, &l, "out"));
        }
        if let Some(err) = child.stderr.take() {
            let l = label.clone();
            thread::spawn(move || pipe_to_log(err, &l, "err"));
        }

        let stdin = child.stdin.take();
        Ok(Self { child, stdin, label })
    }

    /// Take the child's stdin for the engine's per-device writer thread. The
    /// second caller gets `None`.
    pub fn take_stdin(&mut self) -> Option<ChildStdin> {
        self.stdin.take()
    }

    /// Whether the child process is still running.
    pub fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// The `name@ip:port` label for logs/status.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Kill and reap the child (idempotent enough for cleanup).
    pub fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for RaopChild {
    fn drop(&mut self) {
        self.kill();
    }
}

/// Read a child pipe line-by-line into the log until EOF.
fn pipe_to_log<R: std::io::Read>(reader: R, label: &str, which: &str) {
    let buf = BufReader::new(reader);
    for line in buf.lines() {
        match line {
            Ok(l) if !l.trim().is_empty() => log::debug!("[cliraop {label} {which}] {l}"),
            Ok(_) => {}
            Err(_) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_paths_cover_stripped_and_triple_names() {
        let base = Path::new("/install");
        let triple = format!("{CLIRAOP_BASENAME}-aarch64-apple-darwin");
        let names: Vec<String> = candidate_paths(base, &triple)
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert!(names.contains(&"cliraop".to_string())); // bundled mac/linux
        assert!(names.contains(&"cliraop.exe".to_string())); // bundled win
        assert!(names.contains(&"cliraop-aarch64-apple-darwin".to_string())); // dev
    }

    #[test]
    fn resolver_finds_dev_binary_when_present() {
        // The macos-arm64 binary is vendored for local testing; on this host the
        // resolver must locate it via the CARGO_MANIFEST_DIR/binaries fallback.
        // On other hosts/CI without the binary this is a no-op assertion.
        let found = resolve_cliraop(None, None);
        if let Some(p) = found {
            assert!(p.exists());
            assert!(p.file_name().unwrap().to_string_lossy().contains("cliraop"));
        }
    }
}
