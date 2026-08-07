# LP10 firmware notes

Empirical findings from the **FR-23 verification spike** (brief.md §4.3.1), run against
real LP10 hardware during M2. These findings drive FR-22 (master naming), FR-24 (Group
Guard auto-heal) and FR-26 (UI clarity) — do not implement group-integrity behaviour from
assumptions, fill this file in first.

Record for each section: firmware version (`getStatusEx.firmware`), device model/UUID,
date, and the raw request/response text. Redact nothing except the Wi-Fi SSID/BSSID.

---

## 1. Raw `getStatusEx` dump

`GET http://<ip>/httpapi.asp?command=getStatusEx` — solo, as master, as slave.

- [ ] solo
- [ ] master (group of 2+)
- [ ] slave

_Which fields actually flip when the role changes? (`group`, `master_uuid`, `master_ip`,
`slave_mask`, …) — this is the field the poller watches for detach detection._

## 2. Raw `getPlayerStatus` dump

`GET http://<ip>/httpapi.asp?command=getPlayerStatus` — idle, playing via Spotify Connect,
playing as a slave.

- [ ] idle
- [ ] master playing
- [ ] slave playing

_Note the `mode`/source code for each input (Wi-Fi stream, Spotify Connect, AirPlay,
line-in) — FR-19 shows this per device._

## 3. Raw `getSlaveList` dump

`GET http://<ip>/httpapi.asp?command=multiroom:getSlaveList` on the master.

- [ ] empty (solo)
- [ ] one slave
- [ ] two or more slaves

_Is the list authoritative and immediately consistent after a join, or does it lag by a
polling cycle? Does a detached slave disappear at once?_

## 4. Which join variant actually works

Candidates seen in the wild:

- [ ] `ConnectMasterAp:JoinGroupMaster:eth<master_ip>:wifi0.0.0.0` (sent to the **slave**)
- [ ] `ConnectMasterAp:ssid=<hex>:ch=<n>:auth=OPEN:encry=NONE:pwd=:chext=0`
- [ ] `multiroom:SlaveMask` / master-side variants

_Record the exact string that worked, which device it must be sent to, the HTTP response
body, and how long the join takes to become visible in `getSlaveList` (FR-14 retry/backoff
must be tuned to this)._

## 5. Slave-side `multiroom:Ungroup` semantics

- [ ] sent to a **slave** → does only that slave leave, or does the whole group dissolve?
- [ ] sent to the **master** → dissolves the group?
- [ ] does the leaving device keep playing, go silent, or reboot its stream?

_FR-12 offers both "ungroup all" and "per-device leave" — they must map to the right
command target._

## 6. Hex-encoded title check

Some Linkplay firmwares return `Title`/`Artist`/`Album` in `getPlayerStatus` as hex-encoded
UTF-8 rather than plain text.

- [ ] hex-encoded on this firmware? (sample raw value + decoded value)
- [ ] is it always hex, or only for non-ASCII content?
- [ ] any encoding used for `getStatusEx.DeviceName` too?

_Decide the decode heuristic (try hex → UTF-8, fall back to raw) and note false positives
(a title that is legitimately all hex digits)._

## 7. Spotify Connect slave-select behaviour

The core FR-23 question.

- [ ] does the **master** appear in Spotify Connect and play to all slaves in sync?
- [ ] what happens when a **slave** is selected directly — detach only, or full group break?
- [ ] is the slave's Connect entry suppressed/hidden while grouped?
- [ ] same answers for AirPlay and DLNA targeting a slave?
- [ ] how quickly does `getStatusEx` on the detached slave reflect the change?

_This determines whether Group Guard can auto-rejoin silently or must ask the user
(FR-24's ask / always rejoin / never setting)._

## 8. `setDeviceName` with emoji and accents

FR-22 pushes names like `Whole House 🔊` and `(grouped) Cucina`.

- [ ] plain ASCII accepted?
- [ ] accented Latin (`à è ì ò ù`) accepted and echoed back intact?
- [ ] emoji accepted, and does Spotify Connect render it?
- [ ] URL-encoding required? hex-encoding required (see §6)?
- [ ] length limit? characters silently dropped or truncated?
- [ ] does the name survive a reboot? does the original name need restoring on ungroup?

## 9. Detach-visibility latency

- [ ] time from a real detach (source hijack) to `getStatusEx` reporting the new role
- [ ] time from a reboot to the device answering `httpapi.asp` again
- [ ] time from a successful rejoin to `getSlaveList` including the slave

_Sets the floor for `MUSIC_SYNC_POLL_MS` (default 3000) and for FR-24's "detects the change
within one polling cycle" promise — if the firmware lags 10 s, the notification copy and
retry backoff must say so._
