# LP10 firmware notes — FR-23 spike findings

Empirical results from probing real hardware on 2026-08-08:
- **Lofficina-main** — `192.168.10.104`, wired (Ethernet), master unit
- **Lofficina-garden** — `192.168.10.148`, Wi-Fi (Tenda Nova mesh)

Firmware `AR241CE_9243` / `16-0e74fa98-9` (UI) = `AR241CE_9243.16.2` (Luci `DevInfo`).
Model: Arylic LP10 2.0, UPnP UDN `uuid:afcea3b1-ae97-4c5a-9c2e-e2328542154a`.

## ⚠️ Headline finding — the brief's API premise is wrong for this firmware

**These LP10s do NOT expose the classic Linkplay `httpapi.asp`.** Every path/scheme/port
returns `404 Not Found` ("Cannot stat /factory/custom/web/httpapi.asp"), on both wired and
Wi-Fi units. Confirmed against Arylic's own docs and forum: the LP10 is **not** a classic
Linkplay board — it runs a **Libre Wireless LS6** module, and Arylic states LP10 multiroom
is done via **AirPlay 2 / Google Cast only**, with no native-Arylic (Linkplay) multiroom
and no `httpapi`. `developer.arylic.com/httpapi` targets the Up2Stream/Linkplay boards, not
the LP10.

So every command in brief.md §3 (`getStatusEx`, `ConnectMasterAp:JoinGroupMaster`,
`multiroom:*`, `setPlayerCmd:*`) is unavailable. The app cannot be built on that API.

## What these devices DO expose (all verified live)

### A. UPnP / DLNA MediaRenderer — port 49494 (works, unauthenticated)
Standard SOAP. Good for per-device control and identity, NOT for grouping.
- `description.xml` → friendlyName, manufacturer=Arylic, modelName=LP10, stable UDN (uuid).
- `RenderingControl` (`/upnp/control/rendercontrol1`): `GetVolume`→`30`, SetVolume, mute.
- `AVTransport` (`/upnp/control/rendertransport1`): `GetTransportInfo`→`STOPPED`,
  `GetPositionInfo` (track/duration/pos + DIDL metadata), Play/Pause/Next/Prev/Seek/Stop.
- `ConnectionManager`, Tencent `QPlay:1` also present.

### B. Luci control protocol (LibreWireless) — port 7777, TLS — THE native channel ✅
This is the real local control + **native multiroom (DDMS/MRA)** path, and it works.

- **Transport:** TCP `7777`, then TLS **1.2**, **mutual auth** with a client certificate.
- **Client cert:** shipped in the open-source SDK `LibreWireless/LSCommunicator`
  (`Sources/Resources/cert.p12`, PKCS#12 passphrase **`12345678`**; subject "APP Certificate",
  O=Libre Wireless). Server cert is self-signed and NOT validated by the SDK
  (`completionHandler(true)`), so we skip server verification too.
- **Framing (Luci packet):** 10-byte header + UTF-8 JSON payload.
  - Header bytes: `[0..1]` remoteID (0), `[2]` commandType (`1`=READ, `2`=WRITE),
    `[3..4]` command, `[5]` status, `[6..7]` CRC, `[8..9]` dataLen.
  - **Endianness quirk:** requests write command & dataLen **little-endian** (SDK
    `LuciPacketConstructor`); responses carry them **big-endian** (SDK `processLuciData`
    reads `payload[3]<<8 | payload[4]`). Encode/decode accordingly.
  - Multiple frames can arrive concatenated in one TLS record — loop the buffer.
- **Command catalog:** the `MessageBox` enum in the SDK
  (`Sources/LSModules/Communication/MessageBox.swift`). Key IDs:
  - Identity/status: `DevInfo=92`, `FwVersion_Info=5`, `Dev_Name=90`, `DevMACID=91`,
    `RSSI_Indicator=151`, `Region=206`.
  - Playback: `PLAYCNTRL=40`, `PLAYJSON=42`, `TRACK_INFO=44`, `GETPLAYDURATION=49`,
    `CURRSOURCE=50`, `PLAY_STATE=51`, `VOLUME=64`, `Mute_Unmute=63`.
  - **Multiroom (DDMS / MRA):** `MRATrigger=100`, `StandAlone_Mode=101`,
    `DEVICESTATE=103` (QueryMRA), `setZoneID=104`, `DDMS_SSID=105`, `SPEAKER_TYPE=106`,
    `SetupStereo_Pair=108`, `ClientsInMRA=110`, `Master_To_Slave=117`,
    `Slave_To_Master=118`, `SlaveInfo=216`, `Enable_ShareMode=214`, `ZONEVOLUME=219`.
  - Events: `REG_ASYNC_EVENTS=3` / `DREG_ASYNC_EVENTS=4` (subscribe for push updates),
    `DevAttachment_status=38`, `DevDetachment_status=36`.
  - `MessageType`: READ=1, WRITE=2.
- **Proven live:** TLS handshake with the SDK cert succeeds; `REG_ASYNC_EVENTS(3)` acked;
  `DevInfo(92)` returned:
  ```json
  {"macaddress":{"bt":"F4:AB:5C:FC:A8:2F","eth0":"00:E0:3A:00:0A:8A","wlan0":"D8:F7:10:71:86:28"},
   "serialnumber":{"device_serialnumber":"RKARYLLP102625004937"},
   "versioninfo":{"devicefwversion":"AR241CE_9243.16.2","mcuversion":"16"}}
  ```
  Note `eth0` vs `wlan0` MACs → the wired/Wi-Fi distinction the app wants to surface.

### C. Discovery
- **Standard SSDP** (UDP 1900, `ST: urn:schemas-upnp-org:device:MediaRenderer:1`) finds both
  LP10s and yields their `description.xml` (manufacturer=Arylic, model=LP10, uuid). This is
  what our scanner should confirm candidates against (NOT `getStatusEx`).
- **DDMS M-SEARCH** (the SDK's own discovery): UDP **1800**,
  `ST: urn:schemas-upnp-org:device:DDMSServer:1`; replies carry `PORT:` (the Luci port) and
  `SOURCE_LIST:` (capabilities). Use this to find the Luci endpoint + confirm DDMS support.
- These units do **not** advertise `_linkplay._tcp` mDNS.

### D. Google Cast — "Activated" (the thing the brief wanted to replace). Not our control path.

### E. Other open ports (context)
`22, 23, 80(static web only), 7777(Luci/TLS), 9090(WebSocket++ /w TIDAL cert — cloud/GCast
relay, ignore), 49494(UPnP), 1000/2018/2345/5000/5555/9090/9095/10001/42069(vendor/debug)`.

## Implication for the app (architecture pivot)

The Linkplay-based plan (M2 client, M4 grouping, M5 guard) must be re-based onto **Luci**:
- Rust needs a TLS client with the embedded client cert (rustls + the p12 → cert/key),
  server-cert verification disabled, TLS 1.2.
- Replace `linkplay/client.rs` `to_query()` HTTP model with a Luci codec (header + JSON),
  command enum = `MessageBox`, READ/WRITE, async-event subscription for the poller.
- Discovery: SSDP MediaRenderer (identity) + DDMS M-SEARCH :1800 (Luci port/caps).
- Grouping (M4): DDMS/MRA commands (`MRATrigger`, `setZoneID`, `Master_To_Slave`,
  `ClientsInMRA`, `ZONEVOLUME`) — exact JSON payload schemas still need capture (next spike:
  create a group in the Arylic/4STREAM app and observe, or trial the MRA commands live).
- Per-device volume/transport/now-playing can use **either** Luci or UPnP; UPnP is simpler
  and already proven, Luci gives push events. Decide during M5.

Reference SDK (authoritative): https://github.com/LibreWireless/LSCommunicator (Swift).

## F. Grouping grammar (DDMS) — recovered 2026-08-08

Reference: `github.com/JohnnyLeone/hass-studioart` `docs/PROTOCOL.md` — a deep RE of the SAME
LibreWireless LUCI protocol (Revox StudioArt, LS9). Every `MessageBox` id maps to its LUCI op
1:1 (op `0x64`=100 `ddms`, `0x67`=103 `ddms status`, `0x68`=104 `groupid`, `0x69`=105 `ssid`,
`0x65/0x66`=101/102 `ooh master/slave`, `0xDB`=219 `zone volume`, `0xD8`=216 `slave info`).

**Group/topology state** is best read from the **DDMS M-SEARCH** (UDP 1800,
`ST: urn:schemas-upnp-org:device:DDMSServer:1`). Each speaker replies with a CRLF KEY:VALUE
banner (verified live):
```
DeviceName, State (S=standalone), NETMODE (ETH0|WLAN), WIFIBAND (ETH|2G), SPEAKERTYPE,
PORT:7777 (Luci), TCPPORT:2020, MRAMode:DDMS, SOURCE_LIST, USN (id)
```
→ NETMODE/WIFIBAND give the **wired vs Wi-Fi** distinction; State/MRAMode give group role.

**Grouping ops** — Luci cmd **100** (`MRATrigger`/`ddms`), WRITE, **ASCII verb payload**
(exact length enforced by firmware): `SETMASTER`(9), `SETSLAVE`(8), `SETFREE`(7),
`JOINTO`(6), `JOINALL`(7), `JOINNEXT`(8), `DROPALL`(7), `DROPME`(6). `SETFREE`/`DROPALL`
= ungroup; `DROPME` = leave. Rejected unless device is "free"; a half-finished pairing
self-heals to free in ~35 s (keepalive `ALIVE`/`MALIVE` timeout, NV `0xCB`).

**How a group actually forms** (the verbs only steer; DDMS runs the link):
1. Master sets `ooh master` (101, multicast `239.255.255.251:3000`), `groupid` (104),
   `ssid` (105), then `SETMASTER` (100). `CONCOUNT`: 1 = stereo pair, **32 = multi-channel
   (whole-house) master**, 0 = clear.
2. Slave discovers the master by SSID/zone-id (DDMS M-SEARCH) and `SETSLAVE`/`JOINTO`/`JOINALL`
   (100); DDMS opens a direct TCP link (master accepts, slave connects), then RTP audio.
3. Per-zone volume in a group = `zone volume control` (219) / `client zone volume` (220).

**Verified live 2026-08-08:** `SETMASTER` then `SETFREE` on the idle wired main unit — both
acked (status=1), left the device standalone (self-consistent). Full multi-step group
formation (master multicast+groupid+ssid then slave join) to be implemented and audio-verified
in M4. `0x67`/103 `ddms status` is a PUSH (subscribe via `REG_ASYNC_EVENTS`=3), not a plain read.

**Per-device control verified live:** `VOLUME`(64) READ→`30`, WRITE `25` set it, restored;
`Mute`(63)→`UNMUTE`, `PLAY_STATE`(51)→`0/1`, `CURRSOURCE`(50), `DevName`(90), `DevInfo`(92 JSON).

## G. Native DDMS grouping is NON-FUNCTIONAL on LP10 — grouping is Cast/AirPlay only (2026-08-08)

Live result: the Luci DDMS group verbs are **accepted but have no effect** on LP10 firmware
`AR241CE_9243.16.2`.
- `MRATrigger=100` `SETMASTER`/`SETSLAVE`/`JOINTO`/`JOINALL`/`SETFREE` all return status=1
  (accepted, not the error status 2), but the DDMS M-SEARCH `State` never leaves `S`
  (standalone), no `ddms status` (103) push ever fires, and `ClientsInMRA`(110)/`SlaveInfo`(216)
  return nothing.
- `setZoneID=104` and `DDMS_SSID=105` **writes get no ack at all** on this firmware.
- Tried with and without master zone-id/ssid/ooh-master setup; both idle→idle. No grouping.

This matches Arylic's official position (product page + forum): **the LP10 does NOT support
native Arylic/Linkplay multiroom; it syncs only via AirPlay 2 or Google Cast.** The LibreWireless
module ships the DDMS command surface (so the LUCI daemon acks the verbs), but Arylic did not
enable the DDMS multiroom engine on the LP10 — multiroom is delegated to Cast/AirPlay.

**Confirmed grouping path = Google Cast:** both units have port **8009** open and advertise
`_googlecast._tcp` (mDNS instance names `LP10-<id>`). "Google Cast: Activated" in the device
settings. AirPlay 2 is the other (Apple-side) path.

### Consequence for the app
- **Per-device control works great** via Luci/UPnP (volume, mute, transport, source,
  now-playing, wired/Wi-Fi) — keep and ship it.
- **"Play the same music everywhere" cannot be done via a direct LP10 LAN grouping API.**
  It requires Google Cast (or AirPlay 2). Cast *group creation* is a Google Home / cloud
  function; the local CASTV2 protocol (TLS 8009, protobuf) can discover and control existing
  Cast devices/groups and cast media, but does not create groups locally in general.
- Options: (a) integrate CASTV2 to discover Cast groups the user made in Google Home and
  target them / show unified now-playing; (b) implement local Cast multizone if the LP10
  exposes it (uncertain, needs a spike); (c) scope the app to a great per-device controller +
  a clear "cast to your group / select in Spotify Connect" hint. Decision pending with user.

## H. Can we create a Cast group locally (no Google Home)? — NO (2026-08-08)

The LP10s run real Chromecast firmware (`1.68.cast_20240119`, Cast setup API on port **8008**
open; `/setup/eureka_info` responds with device info + public_key). But:
- **Creating** a Cast speaker group is a Google Home / Home-Graph (cloud) operation. There is
  no public/known local API to create a group; modern firmware gates group config behind
  cloud device-auth (the eureka public_key). `pychromecast` (the reference Cast lib) can
  *discover and control* existing groups but **cannot create** them. Confirmed via research.
- Native DDMS grouping is disabled on LP10 (§G).
- AirPlay 2 grouping is Apple-sender-driven (macOS/iOS), not something a cross-platform LAN
  app creates, and needs the app to be the audio source (out of scope).

**Net:** true sample-synced multiroom on these LP10s requires Google's cloud (Home app) — which
in this deployment can't even see the devices (the reason for this app) — or native firmware
grouping Arylic didn't enable. Neither is reachable locally.

### The one locally-achievable "play everywhere" (no cloud)
Cast the **same media locally to each LP10 individually** over CASTV2 (TLS 8009, protobuf) —
"parallel cast", not a real group. No Google Home needed. Caveats: (1) NOT sample-synced
(each device plays independently; rooms may drift by fractions of a second to ~1s), (2) works
only for castable content the app can point at (a stream/URL/local file the app serves) —
**not Spotify Connect**, since Spotify casting is driven by the Spotify app, not us.

Open hypothesis (untested, needs ≥2 Wi-Fi units): DDMS may require both speakers on the same
Wi-Fi interface (the doc notes `SO_BINDTODEVICE` to the active interface + WLAN-IP reachability);
the wired master may be why DDMS no-ops. Worth one test when more wireless LP10s are online.
