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
