# MusicSync architecture — LibreWireless Luci (post-pivot)

Authoritative technical spec for the device-control layer after the FR-23 spike
(`docs/firmware-notes.md`). The LP10 does **not** speak Linkplay httpapi; it speaks the
LibreWireless **Luci** protocol. This document is what the implementation follows.

## Transport

- **Luci**: TCP `<ip>:7777`, then **TLS 1.2**, **mutual auth**. Client cert is embedded
  (`src-tauri/src/luci/assets/librewireless_client_cert.pem` + `_key.pem`, RSA-2048, from the
  public LSCommunicator SDK). Server cert is self-signed and **not validated** (custom
  `ServerCertVerifier` that accepts anything; the SDK does the same). Use rustls with
  `dangerous()` verifier + `with_client_auth_cert`. Cipher the device offers:
  `ECDHE-RSA-AES256-GCM-SHA384`.
- One **persistent** connection per device (matches the SDK). On connect: send
  `REG_ASYNC_EVENTS` (cmd 3, WRITE) to receive state pushes; then periodic reads. Reconnect
  with backoff on drop.
- **UPnP fallback** (optional, per-device only): SOAP on `<ip>:49494`
  (`/upnp/control/rendercontrol1` volume/mute, `/upnp/control/rendertransport1` transport +
  `GetPositionInfo` metadata). Simpler than Luci for now-playing; no grouping. Decide per
  feature; Luci is primary.

## Luci frame codec

10-byte header + UTF-8 payload.
```
byte 0..1  remoteID (u16, 0)
byte 2     commandType (1=READ, 2=WRITE)
byte 3..4  command
byte 5     status
byte 6..7  CRC (0 on send)
byte 8..9  dataLen (payload length)
```
- **Encode (request):** command & dataLen **little-endian**.
- **Decode (response):** command & dataLen **big-endian** (`cmd = b[3]<<8 | b[4]`,
  `len = b[8]<<8 | b[9]`), `status = b[5]`. Response `status==1` = OK.
- Multiple frames can arrive concatenated in one TLS read — loop the buffer, need ≥10 bytes
  for a header and ≥`10+len` for a full frame before consuming.

## Command catalog (`MessageBox`)

Full enum from the SDK. Ones the app uses (verified live where noted):
- Identity/status: `DevInfo=92`(JSON: macaddress{bt,eth0,wlan0}, serialnumber, versioninfo ✓),
  `FwVersion=5`(✓ `AR241CE_9243`), `DevName=90`(✓ plain text), `DevMACID=91`(✓ returns IP).
- Playback (all plain-scalar or JSON): `VOLUME=64`(✓ read `30`, write `"25"` set it),
  `Mute_Unmute=63`(✓ `UNMUTE`/`MUTE`), `PLAY_STATE=51`(✓ `0/1`), `CURRSOURCE=50`(✓ int),
  `PlayBackSource=10`, `TRACK_INFO=44`, `GETPLAYDURATION=49`(async push, ms), `PLAYCNTRL=40`,
  `PLAYJSON=42`.
- Multiroom (DDMS): `MRATrigger=100` (ASCII verb — see grouping), `StandAlone_Mode=101`
  (=`ooh master`), `ooh slave=102`, `DEVICESTATE/QueryMRA=103` (=`ddms status`, **push**),
  `setZoneID=104` (=`groupid`), `DDMS_SSID=105` (=`ssid`), `ClientsInMRA=110`,
  `Master_To_Slave=117`, `SlaveInfo=216`, `Enable_ShareMode=214`, `ZONEVOLUME=219`
  (=`zone volume control`), `client zone volume=220`.
- Events: `REG_ASYNC_EVENTS=3`, `DREG_ASYNC_EVENTS=4`, `DevAttachment_status=38`,
  `DevDetachment_status=36`.

Payload conventions observed: scalars are bare ASCII (`"25"`, `"UNMUTE"`), `DevInfo` is JSON.
Group state via DDMS M-SEARCH banner (below), not a plain Luci read.

## Discovery

- **DDMS M-SEARCH** (primary): UDP multicast `239.255.255.250:1800`,
  `ST: urn:schemas-upnp-org:device:DDMSServer:1`. Reply is a CRLF `KEY:VALUE` banner:
  `DeviceName, State (S=standalone / else grouped role), NETMODE (ETH0|WLAN),
  WIFIBAND (ETH|2G|5G), SPEAKERTYPE, PORT (Luci=7777), TCPPORT (2020), MRAMode (DDMS),
  SOURCE_LIST, USN (id), FWVERSION, CAST_MODEL`. This one packet yields identity + Luci port
  + **wired/Wi-Fi** + group topology. Poll it (~every few s) for topology.
- **SSDP MediaRenderer** (identity/uuid): UDP `239.255.255.250:1900`,
  `ST: urn:schemas-upnp-org:device:MediaRenderer:1` → `description.xml` (manufacturer=Arylic,
  modelName=LP10, stable UDN uuid). Use the UDN as the device's stable id (M-SEARCH `USN` is a
  MAC, also stable — pick one and be consistent; prefer UPnP UDN uuid for the device key).
- **Subnet sweep** (fallback): probe `<ip>:7777` TLS reachability, then DDMS M-SEARCH/DevInfo
  to confirm. Confirm candidates via **Luci DevInfo or the DDMS banner**, NOT httpapi.

## Grouping (DDMS) — the whole-house group flow

Group commands = Luci `MRATrigger=100`, WRITE, **ASCII verb payload** (exact length enforced):
`SETMASTER`(9), `SETSLAVE`(8), `SETFREE`(7), `JOINTO`(6), `JOINALL`(7), `JOINNEXT`(8),
`DROPALL`(7), `DROPME`(6). Verbs only *steer*; the DDMS subsystem runs the audio link.

Rules (from RE + live): a device must be **free** to become master/slave (`State:S`); a
half-finished pairing self-heals to free in ~35 s (keepalive timeout). A stereo pair is
limited to 1 partner; whole-house master uses `CONCOUNT:32`.

Group formation sequence (to implement + **audio-verify live in M4**, refining as needed):
1. **Master** (`ip_m`): `ooh master`(101) → sets DDMS multicast `239.255.255.251:3000`;
   `groupid`(104) + `ssid`(105) to define the zone; `MRATrigger=100` `"SETMASTER"`.
   The master then advertises a DDMS banner (`SSID`, `IP:<master ip>`, `PORT`, `CONCOUNT:32`,
   `MRAMode`, `State`) for slaves to find.
2. **Each slave**: `MRATrigger=100` `"SETSLAVE"` (or `"JOINTO"`/`"JOINALL"`), given the same
   `groupid`/`ssid`; DDMS discovers the master by SSID/zone-id and opens the RTP link.
3. **Ungroup**: `MRATrigger=100` `"SETFREE"` on each member (or `"DROPALL"` on master,
   `"DROPME"` on a leaving slave). Verify via DDMS M-SEARCH `State` returning to `S`.
4. **Per-member volume in a group**: `ZONEVOLUME=219`.

Note: exact `groupid`/`ssid`/`ooh master` payload JSON is not yet field-captured — M4's first
task is a careful live derivation (both LP10s available at 192.168.10.104 wired /
192.168.10.148 Wi-Fi), using DDMS M-SEARCH `State` as the success signal and `SETFREE` +
the ~35 s self-heal as the safety net. `192.168.10.148` is often actively playing — expect
grouping to interrupt it; that's fine during a deliberate test.

## Rust module layout

```
src-tauri/src/
  luci/
    mod.rs
    cert.rs         include_bytes! the two PEM assets
    tls.rs          rustls ClientConfig: TLS1.2, dangerous no-verify, client auth cert
    codec.rs        frame encode/decode (LE request / BE response)
    messagebox.rs   MessageBox enum + MessageType
    client.rs       LuciClient: connect, send(cmd,type,payload), request/reply correlation,
                    async-event stream (channel of (MessageBox,status,payload))
    model.rs        typed parses: DevInfo, PlaybackSnapshot, DdmsBanner, GroupState
  discovery.rs      DDMS M-SEARCH + SSDP MediaRenderer + sweep; DeviceCandidate
  poller.rs         per-device persistent LuciClient + async events + periodic reads +
                    DDMS M-SEARCH topology; emits device-updated/device-offline/group-changed
  group.rs          create_group / delete_group / add_member / remove_member (DDMS sequence)
  upnp.rs           optional SOAP fallback (per-device transport/metadata)
  store.rs state.rs error.rs commands/{auth,devices,groups,playback,settings}.rs
```

Device identity: `uuid` (UPnP UDN) primary key; also store `usn` (MAC) and current `ip`.
`SavedDevice { uuid, usn, ip, alias, net_mode, last_seen, pinned_manual }`.

`DeviceSnapshot` (camelCase to JS): `uuid, ip, name, alias, online, netMode(ethernet|wifi),
wifiBand, model, firmware, role(solo|master|slave), groupId?, masterUuid?, volume, mute,
source, playState, track{title,artist,album,durationMs,positionMs}?, raw(debug)`.

## Frontend

- `lib/luci`-agnostic: `lib/tauri/commands.ts` + `events.ts`; stores
  `devices.svelte.ts`, `groups.svelte.ts`, `scan.svelte.ts`.
- **Groups page** (explicit management, per user request): create a group (name it, pick a
  master, check members), delete a group, add/remove members from an existing group, live
  topology tree (master ⇒ slaves) with per-member volume + wired/Wi-Fi badge, and clear
  master highlight. Optimistic where safe; group mutations show pending/among retry.
- Device cards: wired/Wi-Fi icon + signal (RSSI where available), role badge, volume, source,
  now-playing.
