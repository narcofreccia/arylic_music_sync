//! The `MessageBox` command catalog (from the LSCommunicator SDK's
//! `MessageBox.swift`) and the READ/WRITE `MessageType`.
//!
//! Only the ids the app actually references are named; the rest of the catalog
//! is documented in `docs/firmware-notes.md` and can be added as R2–R4 need
//! them. [`MessageBox::try_from`] decodes an incoming command id, so an
//! unrecognised push is reported (as the raw number) rather than silently
//! mis-parsed.

/// READ vs WRITE — byte 2 of the frame header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MessageType {
    Read = 1,
    Write = 2,
}

impl MessageType {
    pub fn id(self) -> u8 {
        self as u8
    }
}

/// A Luci command id. `#[repr(u16)]` so `self as u16` is the wire value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u16)]
pub enum MessageBox {
    /// Subscribe to async state pushes (send WRITE on connect).
    RegAsyncEvents = 3,
    DregAsyncEvents = 4,
    FwVersion = 5,
    PlayBackSource = 10,
    DevDetachmentStatus = 36,
    DevAttachmentStatus = 38,
    Playcntrl = 40,
    Playjson = 42,
    TrackInfo = 44,
    /// Play position, pushed in ms.
    GetPlayDuration = 49,
    CurrSource = 50,
    PlayState = 51,
    MuteUnmute = 63,
    Volume = 64,
    DevName = 90,
    DevMacId = 91,
    DevInfo = 92,
    MraTrigger = 100,
    StandAloneMode = 101,
    OohSlave = 102,
    /// QueryMRA — pushed, not a plain read.
    DeviceState = 103,
    SetZoneId = 104,
    DdmsSsid = 105,
    SpeakerType = 106,
    SetupStereoPair = 108,
    ClientsInMra = 110,
    MasterToSlave = 117,
    SlaveToMaster = 118,
    RssiIndicator = 151,
    Region = 206,
    EnableShareMode = 214,
    SlaveInfo = 216,
    ZoneVolume = 219,
    ClientZoneVolume = 220,
}

impl MessageBox {
    /// The wire command id.
    pub fn id(self) -> u16 {
        self as u16
    }
}

impl TryFrom<u16> for MessageBox {
    /// The unrecognised id, so the caller can log it.
    type Error = u16;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        use MessageBox::*;
        Ok(match value {
            3 => RegAsyncEvents,
            4 => DregAsyncEvents,
            5 => FwVersion,
            10 => PlayBackSource,
            36 => DevDetachmentStatus,
            38 => DevAttachmentStatus,
            40 => Playcntrl,
            42 => Playjson,
            44 => TrackInfo,
            49 => GetPlayDuration,
            50 => CurrSource,
            51 => PlayState,
            63 => MuteUnmute,
            64 => Volume,
            90 => DevName,
            91 => DevMacId,
            92 => DevInfo,
            100 => MraTrigger,
            101 => StandAloneMode,
            102 => OohSlave,
            103 => DeviceState,
            104 => SetZoneId,
            105 => DdmsSsid,
            106 => SpeakerType,
            108 => SetupStereoPair,
            110 => ClientsInMra,
            117 => MasterToSlave,
            118 => SlaveToMaster,
            151 => RssiIndicator,
            206 => Region,
            214 => EnableShareMode,
            216 => SlaveInfo,
            219 => ZoneVolume,
            220 => ClientZoneVolume,
            other => return Err(other),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_type_ids() {
        assert_eq!(MessageType::Read.id(), 1);
        assert_eq!(MessageType::Write.id(), 2);
    }

    #[test]
    fn id_round_trips_through_try_from() {
        for mb in [
            MessageBox::RegAsyncEvents,
            MessageBox::DevInfo,
            MessageBox::Volume,
            MessageBox::MuteUnmute,
            MessageBox::PlayState,
            MessageBox::CurrSource,
            MessageBox::TrackInfo,
            MessageBox::GetPlayDuration,
            MessageBox::DevName,
            MessageBox::DeviceState,
            MessageBox::ZoneVolume,
            MessageBox::SlaveInfo,
        ] {
            assert_eq!(MessageBox::try_from(mb.id()), Ok(mb));
        }
    }

    #[test]
    fn known_ids_match_the_spec() {
        assert_eq!(MessageBox::DevInfo.id(), 92);
        assert_eq!(MessageBox::Volume.id(), 64);
        assert_eq!(MessageBox::MuteUnmute.id(), 63);
        assert_eq!(MessageBox::PlayState.id(), 51);
        assert_eq!(MessageBox::CurrSource.id(), 50);
        assert_eq!(MessageBox::RegAsyncEvents.id(), 3);
        assert_eq!(MessageBox::MraTrigger.id(), 100);
    }

    #[test]
    fn unknown_id_returns_the_raw_value() {
        assert_eq!(MessageBox::try_from(9999), Err(9999));
        assert_eq!(MessageBox::try_from(0), Err(0));
    }
}
