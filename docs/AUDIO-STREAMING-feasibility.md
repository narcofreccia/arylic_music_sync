# Feasibility: Spotify-everywhere, synced, without Google Home

Research spike (2026-08-08) into whether MusicSync can capture a Spotify stream itself and
send it to all LP10s in sync, bypassing Google Home / cloud speaker groups. Verdict:
**achievable, but it turns the app into an audio server, and Windows + Spotify ToS are the
real constraints.**

## The pipeline

```
Spotify app → (pick "MusicSync" in Connect) → librespot → PCM → AirPlay/RAOP multi-sender → N× LP10
                                                                  └ sync enforced here (shared clock)
```

## Piece 1 — Spotify capture: librespot (SOLID)
- `librespot-org/librespot` — mature, MIT, active (6.9k★). Advertises as a Spotify Connect
  endpoint "MusicSync"; user picks it in their Spotify app. Decodes to PCM we can tap via a
  custom `Sink` (in-process) or a pipe (sidecar). **Spotify Premium required.**
- **Why capture-and-resend is the only path:** Spotify Connect allows ONE active session per
  account — you cannot fan Connect out to N speakers. No sanctioned API gives raw PCM (DRM).
- **⚠️ ToS risk:** librespot is a reverse-engineered Spotify client; Spotify's ToS forbid it
  (their own README warns). Embedding it in a distributed product is a conscious legal/product
  risk (commonly done, never blessed; theoretically risks the user's account). Not a technical
  blocker — a decision.

## Piece 2 — Synced distribution: the hard part
True cross-room sync needs a shared timebase (PTP/NTP). Options for the closed-firmware LP10:

| Path | Sync quality | Platforms | Maturity | Notes |
|---|---|---|---|---|
| **AirPlay 2** (OwnTone sidecar) | Tight (PTP) | **mac/Linux only** | Mature (OwnTone 2.5k★, GPL-2) | The one proven open AP2 synced sender. No Windows. |
| **AirPlay 1 / RAOP** (libraop sidecar) | Good (shared-clock) | **Win/mac/Linux** | Mature (libraop, since 2016) | Cross-platform incl. Windows. AP1 = less robust than AP2. LP10 must accept RAOP senders (very likely; test). |
| Pure-Rust AP2 crates | — | all | **Alpha (`todo!()` stubs)** | Not shippable. |
| Parallel DLNA / Cast + delay slider | **None** (0–4s drift + clock drift) | all (pure Rust) | Easy | Only OK for acoustically-separate rooms. |
| Snapcast | Excellent | — | — | **Impossible** — needs a client installed on each speaker (closed firmware). |

- Latency: AirPlay adds ~2s uniform buffer — fine for music, speaker-to-speaker stays locked.
- LP10 confirmed a genuine AirPlay 2 receiver; Arylic's own multiroom path is AirPlay 2 / Cast.

## Integration: sidecar pattern (like the tide apps)
Bundle the C sender (OwnTone or libraop) as a Tauri `externalBin`, spawned by the Rust shell —
same shape as tide_share/tide_reel's PyInstaller sidecar (`src-tauri/src/sidecar.rs`,
`scripts/build_sidecar.sh`). OwnTone is driven over its HTTP JSON API; libraop over CLI/stdio.
GPL stays contained as long as it's a **separate process** (mere aggregation — our code doesn't
become GPL); no static linking.

## Recommendation — three shippable shapes

1. **Cross-platform synced (best balance):** librespot → **libraop (RAOP) sidecar** → LP10s.
   Works on Windows/mac/Linux with real shared-clock sync. AirPlay 1, not 2 — needs a hardware
   test that the LP10 accepts RAOP senders. ~4–8 weeks.
2. **Best sync, mac/Linux only:** librespot → **OwnTone (AirPlay 2)** sidecar. Tight AP2 sync,
   no Windows. ~4–8 weeks.
3. **Trivial fallback, everywhere:** parallel DLNA + per-room delay slider. All-Rust, no real
   sync — label it "same music everywhere," good only for separated rooms.

**Blocking caveats for any synced option:** Spotify Premium; user picks "MusicSync" in Spotify;
ToS gray area; the app grows from a LAN controller into a bundled audio server (new C sidecars,
per-platform build/CI, GPL conveyance). Validate LP10 RAOP/AP2 pairing on real hardware before
committing.

Sources: librespot, owntone-server, philippe44/libraop, music-assistant/airplay-cli,
lmcgartland/airplay2-rs (alpha), Arylic LP10 docs, tide_share sidecar pattern. Full URLs in the
spike transcripts.
