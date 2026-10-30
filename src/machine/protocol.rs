//! Brother PP1 ("Skitch") Bluetooth LE protocol: message framing and payloads.
//!
//! Learned from Respira's BrotherPP1Service
//! (https://github.com/jhbruhn/respira, Apache-2.0, Copyright Jan-Henrik Bruhn),
//! which reverse-engineered Brother's Artspira app. This module is transport-free
//! so it can be unit tested; `skitch.rs` sends these bytes over BLE.
//!
//! Every request is a big-endian u16 command id followed by a payload, written
//! to the write characteristic. Replies are read from the read characteristic:
//! `[cmd_hi, cmd_lo, status, ...]`.

pub const SERVICE_UUID: &str = "a76eb9e0-f3ac-4990-84cf-3a94d2426b2b";
pub const WRITE_CHAR_UUID: &str = "a76eb9e2-f3ac-4990-84cf-3a94d2426b2b";
pub const READ_CHAR_UUID: &str = "a76eb9e1-f3ac-4990-84cf-3a94d2426b2b";

pub mod cmd {
    pub const MACHINE_INFO: u16 = 0x0000;
    pub const MACHINE_STATE: u16 = 0x0001;
    pub const SERVICE_COUNT: u16 = 0x0100;
    pub const PATTERN_UUID_REQUEST: u16 = 0x0702;
    pub const MASK_TRACE: u16 = 0x0704;
    pub const LAYOUT_SEND: u16 = 0x0705;
    pub const EMB_SEWING_INFO_REQUEST: u16 = 0x0706;
    pub const PATTERN_SEWING_INFO: u16 = 0x0707;
    pub const EMB_SEWING_DATA_DELETE: u16 = 0x0708;
    pub const NEEDLE_MODE_INSTRUCTIONS: u16 = 0x0709;
    pub const EMB_UUID_SEND: u16 = 0x070a;
    pub const START_SEWING: u16 = 0x070e;
    pub const REGULAR_INSPECTION: u16 = 0x0103;
    pub const MACHINE_SETTING_INFO: u16 = 0x0c02;
    pub const SEND_DATA_INFO: u16 = 0x1200;
    pub const SEND_DATA: u16 = 0x1201;
    pub const CLEAR_ERROR: u16 = 0x1300;
    pub const RESUME_FLAG_REQUEST: u16 = 0x070b;
    pub const RESUME: u16 = 0x070c;
    pub const HOOP_AVOIDANCE: u16 = 0x070f;
    pub const MASK_TRACE_1: u16 = 0x0710;
    pub const EMB_ORG_POINT: u16 = 0x0800;
    pub const FIRM_UPDATE_START: u16 = 0x0b00;
    pub const SET_SETTING_REST: u16 = 0x0c00;
    pub const SET_SETTING_SEND: u16 = 0x0c01;
    pub const ERROR_LOG_REPLY: u16 = 0x1301;

    /// A command's name (Respira's), for logs and capture listings.
    pub fn name(c: u16) -> Option<&'static str> {
        Some(match c {
            MACHINE_INFO => "MACHINE_INFO",
            MACHINE_STATE => "MACHINE_STATE",
            SERVICE_COUNT => "SERVICE_COUNT",
            REGULAR_INSPECTION => "REGULAR_INSPECTION",
            PATTERN_UUID_REQUEST => "PATTERN_UUID_REQUEST",
            MASK_TRACE => "MASK_TRACE",
            LAYOUT_SEND => "LAYOUT_SEND",
            EMB_SEWING_INFO_REQUEST => "EMB_SEWING_INFO_REQUEST",
            PATTERN_SEWING_INFO => "PATTERN_SEWING_INFO",
            EMB_SEWING_DATA_DELETE => "EMB_SEWING_DATA_DELETE",
            NEEDLE_MODE_INSTRUCTIONS => "NEEDLE_MODE_INSTRUCTIONS",
            EMB_UUID_SEND => "EMB_UUID_SEND",
            RESUME_FLAG_REQUEST => "RESUME_FLAG_REQUEST",
            RESUME => "RESUME",
            START_SEWING => "START_SEWING",
            HOOP_AVOIDANCE => "HOOP_AVOIDANCE",
            MASK_TRACE_1 => "MASK_TRACE_1",
            EMB_ORG_POINT => "EMB_ORG_POINT",
            FIRM_UPDATE_START => "FIRM_UPDATE_START",
            SET_SETTING_REST => "SET_SETTING_REST",
            SET_SETTING_SEND => "SET_SETTING_SEND",
            MACHINE_SETTING_INFO => "MACHINE_SETTING_INFO",
            SEND_DATA_INFO => "SEND_DATA_INFO",
            SEND_DATA => "SEND_DATA",
            CLEAR_ERROR => "CLEAR_ERROR",
            ERROR_LOG_REPLY => "ERROR_LOG_REPLY",
            _ => return None,
        })
    }
}

/// Pattern data is uploaded in chunks of this many bytes.
pub const CHUNK_SIZE: usize = 500;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Initial,
    LowerThread,
    Idle,
    ReadyToSew,
    ReceivingData,
    WaitingForMaskTrace,
    MaskTracing,
    MaskTraceComplete,
    Sewing,
    Complete,
    Interrupted,
    WaitingForColorChange,
    Paused,
    Stopped,
    HoopAvoidance,
    HoopAvoidanceInProgress,
    RlReceiving,
    RlReceived,
    Unknown(u8),
}

impl Status {
    pub fn from_byte(b: u8) -> Status {
        use Status::*;
        match b {
            0x00 => Initial,
            0x01 => LowerThread,
            0x10 => Idle,
            0x11 => ReadyToSew,
            0x12 => ReceivingData,
            0x20 => WaitingForMaskTrace,
            0x21 => MaskTracing,
            0x22 => MaskTraceComplete,
            0x30 => Sewing,
            0x31 => Complete,
            0x32 => Interrupted,
            0x40 => WaitingForColorChange,
            0x41 => Paused,
            0x42 => Stopped,
            0x50 => HoopAvoidance,
            0x51 => HoopAvoidanceInProgress,
            0x60 => RlReceiving,
            0x61 => RlReceived,
            other => Unknown(other),
        }
    }

    pub fn byte(self) -> u8 {
        use Status::*;
        match self {
            Initial => 0x00,
            LowerThread => 0x01,
            Idle => 0x10,
            ReadyToSew => 0x11,
            ReceivingData => 0x12,
            WaitingForMaskTrace => 0x20,
            MaskTracing => 0x21,
            MaskTraceComplete => 0x22,
            Sewing => 0x30,
            Complete => 0x31,
            Interrupted => 0x32,
            WaitingForColorChange => 0x40,
            Paused => 0x41,
            Stopped => 0x42,
            HoopAvoidance => 0x50,
            HoopAvoidanceInProgress => 0x51,
            RlReceiving => 0x60,
            RlReceived => 0x61,
            Unknown(b) => b,
        }
    }

    /// What the machine is doing, in a few words.
    pub fn label(self) -> &'static str {
        use Status::*;
        match self {
            Initial => "Needs homing",
            LowerThread => "Winding the bobbin",
            Idle => "Ready",
            ReadyToSew => "Ready to sew",
            ReceivingData => "Receiving the design",
            WaitingForMaskTrace => "Waiting to start the trial run",
            MaskTracing => "Trial run",
            MaskTraceComplete => "Trial run done",
            Sewing => "Sewing",
            Complete => "Finished",
            Interrupted => "Interrupted",
            WaitingForColorChange => "Waiting for a thread change",
            Paused => "Paused",
            Stopped => "Stopped",
            HoopAvoidance | HoopAvoidanceInProgress => "Moving the hoop clear",
            RlReceiving | RlReceived => "Receiving",
            Unknown(_) => "Busy",
        }
    }
}

/// The error byte when there's no error.
pub const NO_ERROR: u8 = 0xdd;

/// A machine error: what it is and what to do about it (from Artspira's
/// SewingMachineError, as Respira describes them).
#[derive(Clone, Copy, Debug)]
pub struct ErrorHelp {
    pub title: &'static str,
    pub description: &'static str,
    pub steps: &'static [&'static str],
    /// A normal step rather than a fault (the homing the machine asks for).
    pub informational: bool,
    /// Thread errors: how many stitches to go back before sewing on, so
    /// the gap the broken thread left is sewn over.
    pub rollback: Option<u16>,
}

/// What an error byte means (`None` for [`NO_ERROR`]).
pub fn error_help(code: u8) -> Option<ErrorHelp> {
    let e = |title, description, steps| ErrorHelp {
        title,
        description,
        steps,
        informational: false,
        rollback: None,
    };
    Some(match code {
        NO_ERROR => return None,
        0x00 => e(
            "The needle is down",
            "The needle needs to be up before the machine can go on.",
            &["Press the needle position button to raise the needle."],
        ),
        0x01 => e(
            "Safety error",
            "The machine sensed something wrong where it sews.",
            &[
                "Remove the thread from the top of the fabric, then remove the needle.",
                "Remove the thread from under the fabric and clean any thread out of the bobbin case.",
                "Check the bobbin case for scratches or dirt.",
                "Put the embroidery needle back in.",
                "Check that the bobbin is in the right way round.",
            ],
        ),
        0x02 => e(
            "Bobbin winder safety stop",
            "The bobbin winder's safety device stopped it.",
            &["Check whether the thread is tangled."],
        ),
        0x03 => e(
            "Bobbin winder",
            "There's a problem with the bobbin thread.",
            &["Slide the bobbin winder shaft toward the front."],
        ),
        0x10..=0x1c => e(
            "Restart the machine",
            "Something went wrong inside the machine.",
            &[
                "Turn the machine off, then on again.",
                "If it keeps happening, note the error code and contact Brother support.",
            ],
        ),
        0x20 => e(
            "Needle plate cover",
            "The needle plate cover isn't on properly.",
            &[
                "Put the needle plate cover back on.",
                "Check the bobbin case (seated right, no scratches), then put the cover back on.",
            ],
        ),
        0x21 => e(
            "Threading lever",
            "The needle threading lever isn't back in place.",
            &["Push the needle threading lever back up to where it rests."],
        ),
        0x60 => ErrorHelp {
            rollback: Some(6),
            ..e(
                "Upper thread",
                "The top thread broke or came out.",
                &[
                    "Rethread the top thread.",
                    "If it keeps happening, change the embroidery needle and rethread.",
                ],
            )
        },
        0x61 => ErrorHelp {
            rollback: Some(2),
            ..e(
                "Bobbin thread",
                "The bobbin is almost empty.",
                &[
                    "Wind a bobbin and put it in.",
                    "Pull the bobbin thread up before you go on.",
                ],
            )
        },
        0x62 => ErrorHelp {
            rollback: Some(21),
            ..e(
                "Upper thread at the start",
                "The top thread wasn't caught when sewing started.",
                &[
                    "Press the Accept button on the machine.",
                    "Rethread the top thread.",
                    "If it keeps happening, change the embroidery needle and rethread.",
                ],
            )
        },
        0x63 => e(
            "Thread wiper",
            "The thread wiper reported a problem.",
            &["Press the Accept button on the machine."],
        ),
        0x70 => e(
            "Wrong frame",
            "This embroidery frame can't sew this design.",
            &["Use a frame the design fits in."],
        ),
        0x71 => e(
            "No frame",
            "There's no embroidery frame on the machine.",
            &["Attach the embroidery frame."],
        ),
        0x72 => ErrorHelp {
            informational: true,
            ..e(
                "Home the machine",
                "The machine needs to find its starting position.",
                &[
                    "Take the embroidery frame off the machine.",
                    "Press the Accept button on the machine.",
                    "Wait while the arm moves to its starting position.",
                    "Attach the frame again.",
                ],
            )
        },
        0x80 => e(
            "Maintenance due",
            "The machine has sewn enough that a service is recommended.",
            &["Contact a Brother service center when you can; you can keep sewing."],
        ),
        0x98 => e(
            "Can't change that now",
            "The machine refused a change to the stitch position.",
            &["Try again once the machine is paused."],
        ),
        _ => e(
            "Machine error",
            "The machine reported a problem ZigZag doesn't know.",
            &[
                "Check the machine's lights and the error code.",
                "Turn the machine off and on again if it won't clear.",
            ],
        ),
    })
}

pub fn frame(cmd: u16, payload: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(2 + payload.len());
    v.extend_from_slice(&cmd.to_be_bytes());
    v.extend_from_slice(payload);
    v
}

/// SEND_DATA_INFO payload: type 3, total length, 16-bit byte-sum checksum.
pub fn data_info(data: &[u8]) -> Vec<u8> {
    let checksum = data.iter().fold(0u32, |a, &b| a + b as u32) as u16;
    let mut p = vec![0x03];
    p.extend_from_slice(&(data.len() as u32).to_le_bytes());
    p.extend_from_slice(&checksum.to_le_bytes());
    p
}

/// SEND_DATA payloads: offset (u32 LE), chunk, 8-bit byte-sum checksum.
pub fn data_chunks(data: &[u8]) -> Vec<Vec<u8>> {
    data.chunks(CHUNK_SIZE)
        .enumerate()
        .map(|(i, chunk)| {
            let mut p = Vec::with_capacity(chunk.len() + 5);
            p.extend_from_slice(&((i * CHUNK_SIZE) as u32).to_le_bytes());
            p.extend_from_slice(chunk);
            p.push(chunk.iter().fold(0u8, |a, &b| a.wrapping_add(b)));
            p
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub move_x: i16,
    pub move_y: i16,
    /// Scale in percent (100 = unscaled).
    pub size_x: i16,
    pub size_y: i16,
    pub rotate: i16,
    pub flip: u8,
    /// 0 = 70 mm frame, 1 = 100 mm frame.
    pub frame: u8,
    pub top_left: (i16, i16),
    pub bottom_right: (i16, i16),
    pub center: (i16, i16),
}

impl Layout {
    /// Center a pattern with the given stitch bounds (units) in the hoop, picking
    /// the 70 mm frame when it fits (Artspira's GetRealFrame logic).
    pub fn centered(bounds: (i32, i32, i32, i32)) -> Layout {
        let (l, t, r, b) = bounds;
        let (cx, cy) = ((l + r) as f64 / 2.0, (t + b) as f64 / 2.0);
        Self::moved(bounds, (-cx).round(), (-cy).round())
    }

    /// A pattern whose stitches are already where they go, relative to the
    /// hoop's center (as placed in the studio's hoop).
    pub fn as_placed(bounds: (i32, i32, i32, i32)) -> Layout {
        Self::moved(bounds, 0.0, 0.0)
    }

    /// A pattern moved by (`mx`, `my`) units: the 70 mm frame when all of it
    /// lands inside one.
    fn moved(bounds: (i32, i32, i32, i32), mx: f64, my: f64) -> Layout {
        let (l, t, r, b) = bounds;
        let (cx, cy) = ((l + r) as f64 / 2.0, (t + b) as f64 / 2.0);
        let inside = |lo: f64, hi: f64| lo >= -350.0 && hi <= 350.0;
        let fits70 = inside(l as f64 + mx, r as f64 + mx) && inside(t as f64 + my, b as f64 + my);
        Layout {
            move_x: mx as i16,
            move_y: my as i16,
            size_x: 100,
            size_y: 100,
            rotate: 0,
            flip: 0,
            frame: if fits70 { 0 } else { 1 },
            top_left: (
                (l as f64 + mx).round() as i16,
                (t as f64 + my).round() as i16,
            ),
            bottom_right: (
                (r as f64 + mx).round() as i16,
                (b as f64 + my).round() as i16,
            ),
            center: ((cx + mx).round() as i16, (cy + my).round() as i16),
        }
    }

    pub fn payload(&self) -> [u8; 24] {
        let mut p = [0u8; 24];
        let mut put = |o: usize, v: i16| p[o..o + 2].copy_from_slice(&v.to_le_bytes());
        put(0, self.move_x);
        put(2, self.move_y);
        put(4, self.size_x);
        put(6, self.size_y);
        put(8, self.rotate);
        put(12, self.top_left.0);
        put(14, self.top_left.1);
        put(16, self.bottom_right.0);
        put(18, self.bottom_right.1);
        put(20, self.center.0);
        put(22, self.center.1);
        p[10] = self.flip;
        p[11] = self.frame;
        p
    }
}

#[derive(Clone, Debug)]
pub struct MachineInfo {
    pub serial: String,
    pub model: String,
    pub software_version: String,
    pub bluetooth_version: u16,
    /// Max embroidery area, units.
    pub max_width: u16,
    pub max_height: u16,
    pub mac: String,
}

/// Parse a MACHINE_INFO reply (including its 2-byte command header).
pub fn parse_machine_info(resp: &[u8]) -> Option<MachineInfo> {
    let d = resp.get(2..)?;
    if d.len() < 50 {
        return None;
    }
    let ascii = |r: std::ops::Range<usize>| {
        String::from_utf8_lossy(&d[r])
            .trim_end_matches('\0')
            .trim()
            .to_string()
    };
    let sw = u16::from_be_bytes([d[0], d[1]]);
    Some(MachineInfo {
        serial: ascii(2..11),
        model: ascii(39..50),
        software_version: format!("{:.2}.{}", sw as f64 / 100.0, d[35]),
        bluetooth_version: u16::from_be_bytes([d[24], d[25]]),
        max_width: u16::from_le_bytes([d[29], d[30]]),
        max_height: u16::from_le_bytes([d[31], d[32]]),
        mac: d[16..22]
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(":"),
    })
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Progress {
    pub current_stitch: u16,
    pub current_time_s: i16,
    pub stop_time_s: i16,
    pub x: i16,
    pub y: i16,
}

pub fn parse_progress(resp: &[u8]) -> Option<Progress> {
    let d = resp.get(2..)?;
    if d.len() < 10 {
        return None;
    }
    let i16at = |o: usize| i16::from_le_bytes([d[o], d[o + 1]]);
    Some(Progress {
        current_stitch: u16::from_le_bytes([d[0], d[1]]),
        current_time_s: i16at(2),
        stop_time_s: i16at(4),
        x: i16at(6),
        y: i16at(8),
    })
}

/// The pattern loaded on the machine (EMB_SEWING_INFO_REQUEST).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PatternInfo {
    pub bounds: (i16, i16, i16, i16),
    /// Seconds.
    pub total_time_s: u16,
    pub total_stitches: u16,
    /// Stitches a minute.
    pub speed: u16,
}

pub fn parse_pattern_info(resp: &[u8]) -> Option<PatternInfo> {
    let d = resp.get(2..)?;
    if d.len() < 14 {
        return None;
    }
    let i = |o: usize| i16::from_le_bytes([d[o], d[o + 1]]);
    let u = |o: usize| u16::from_le_bytes([d[o], d[o + 1]]);
    Some(PatternInfo {
        bounds: (i(0), i(2), i(4), i(6)),
        total_time_s: u(8),
        total_stitches: u(10),
        speed: u(12),
    })
}

/// SERVICE_COUNT: (stitches since the last service, stitches ever).
pub fn parse_service_count(resp: &[u8]) -> Option<(u32, u32)> {
    let d = resp.get(2..)?;
    if d.len() < 8 {
        return None;
    }
    let u = |o: usize| u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]]);
    Some((u(0), u(4)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framing_and_checksums() {
        assert_eq!(frame(cmd::SEND_DATA_INFO, &[1, 2]), vec![0x12, 0x00, 1, 2]);
        let data: Vec<u8> = (0..1200u32).map(|i| (i % 251) as u8).collect();
        let info = data_info(&data);
        assert_eq!(info[0], 3);
        assert_eq!(u32::from_le_bytes(info[1..5].try_into().unwrap()), 1200);
        let sum: u32 = data.iter().map(|&b| b as u32).sum();
        assert_eq!(u16::from_le_bytes([info[5], info[6]]), sum as u16);
        let chunks = data_chunks(&data);
        assert_eq!(chunks.len(), 3);
        assert_eq!(
            u32::from_le_bytes(chunks[2][0..4].try_into().unwrap()),
            1000
        );
        assert_eq!(chunks[2].len(), 4 + 200 + 1);
        let s: u8 = data[1000..].iter().fold(0u8, |a, &b| a.wrapping_add(b));
        assert_eq!(*chunks[2].last().unwrap(), s);
    }

    #[test]
    fn layout_centers_and_picks_frame() {
        let l = Layout::centered((-200, -100, 200, 100));
        assert_eq!((l.move_x, l.move_y, l.frame), (0, 0, 0));
        assert_eq!(l.top_left, (-200, -100));
        let big = Layout::centered((-400, -400, 400, 400));
        assert_eq!(big.frame, 1);
        let off = Layout::centered((0, 0, 100, 100));
        assert_eq!((off.move_x, off.move_y), (-50, -50));
        assert_eq!(off.center, (0, 0));
        // As placed: no move; the frame by where it actually lands.
        let placed = Layout::as_placed((100, 200, 300, 400));
        assert_eq!((placed.move_x, placed.move_y), (0, 0));
        assert_eq!(placed.center, (200, 300));
        assert_eq!(placed.frame, 1, "reaches past the 70 mm frame's edge");
        assert_eq!(Layout::as_placed((-300, -300, 300, 300)).frame, 0);
        let p = l.payload();
        assert_eq!(i16::from_le_bytes([p[12], p[13]]), -200);
        assert_eq!(p[4..6], 100i16.to_le_bytes());
    }
}
