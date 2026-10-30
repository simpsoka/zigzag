//! Reading Bluetooth recordings of Artspira talking to a Skitch, to work out
//! commands nobody has documented yet (the machine settings Artspira
//! sends). Reads the two formats phones produce:
//!
//! - Apple PacketLogger (`.pklg`): an iPhone with Apple's Bluetooth logging
//!   profile, recorded live from a Mac with PacketLogger.
//! - btsnoop (`btsnoop_hci.log`): Android's "Bluetooth HCI snoop log".
//!
//! Both hold the raw HCI traffic. From it this picks out the ATT writes,
//! reads and notifications (reassembling ACL fragments and long writes),
//! finds which attribute handles are the Skitch's (the one written with
//! known commands, the one read back), and lists the conversation by
//! command name. Settings replies are compared so a changed byte stands out.

use super::protocol::cmd;
use anyhow::{Result, bail};
use std::collections::HashMap;

/// Which way a message went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    /// From the phone (or computer) to the machine.
    ToMachine,
    FromMachine,
}

/// One ATT operation found in a recording.
#[derive(Clone, Debug, PartialEq)]
pub struct Att {
    /// Seconds from the recording's start.
    pub t: f64,
    pub dir: Dir,
    /// The ATT opcode (0x12 write, 0x52 write without response, 0x0B read
    /// response, 0x1B notification…).
    pub op: u8,
    pub handle: u16,
    pub value: Vec<u8>,
}

/// A Skitch command or reply: its id and what followed it.
#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    pub t: f64,
    pub dir: Dir,
    pub command: u16,
    pub data: Vec<u8>,
}

impl Message {
    pub fn name(&self) -> String {
        cmd::name(self.command)
            .map(str::to_string)
            .unwrap_or_else(|| format!("0x{:04x}", self.command))
    }
}

/// HCI packets: (time in seconds, sent by the host, H4 type, bytes).
type Hci = (f64, bool, u8, Vec<u8>);

fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

fn le32(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

/// btsnoop: `btsnoop\0`, version, datalink; then records.
fn btsnoop(bytes: &[u8]) -> Result<Vec<Hci>> {
    let datalink = be32(&bytes[12..16]);
    if datalink != 1001 && datalink != 1002 {
        bail!("btsnoop datalink {datalink} isn't HCI");
    }
    let mut out = Vec::new();
    let mut at = 16;
    let mut t0 = None;
    while at + 24 <= bytes.len() {
        let incl = be32(&bytes[at + 4..]) as usize;
        let flags = be32(&bytes[at + 8..]);
        let ts = i64::from_be_bytes(bytes[at + 16..at + 24].try_into()?);
        at += 24;
        let Some(data) = bytes.get(at..at + incl) else {
            break;
        };
        at += incl;
        let t = ts as f64 / 1e6;
        let t = t - *t0.get_or_insert(t);
        let sent = flags & 1 == 0;
        let (kind, rest) = if datalink == 1002 {
            match data.split_first() {
                Some((k, r)) => (*k, r.to_vec()),
                None => continue,
            }
        } else {
            // Unencapsulated: bit 1 says command/event, else data.
            let kind = match (flags & 2 != 0, sent) {
                (true, true) => 0x01,
                (true, false) => 0x04,
                (false, _) => 0x02,
            };
            (kind, data.to_vec())
        };
        out.push((t, sent, kind, rest));
    }
    Ok(out)
}

/// PacketLogger: records of length, seconds, microseconds, type, bytes (in
/// either byte order).
fn packetlogger(bytes: &[u8]) -> Result<Vec<Hci>> {
    let n32 = |b: &[u8], big: bool| if big { be32(b) } else { le32(b) };
    let plausible = |big: bool| {
        bytes.len() >= 13 && {
            let len = n32(bytes, big) as usize;
            (9..=0x10000).contains(&len) && len + 4 <= bytes.len()
        }
    };
    let big = if plausible(true) {
        true
    } else if plausible(false) {
        false
    } else {
        bail!("not a PacketLogger file");
    };
    let mut out = Vec::new();
    let mut at = 0;
    let mut t0 = None;
    while at + 13 <= bytes.len() {
        let len = n32(&bytes[at..], big) as usize;
        if len < 9 || at + 4 + len > bytes.len() {
            break;
        }
        let secs = n32(&bytes[at + 4..], big) as f64;
        let usecs = n32(&bytes[at + 8..], big) as f64;
        let kind = bytes[at + 12];
        let data = bytes[at + 13..at + 4 + len].to_vec();
        at += 4 + len;
        let t = secs + usecs / 1e6;
        let t = t - *t0.get_or_insert(t);
        // 0x02 ACL sent, 0x03 ACL received; others aren't data.
        match kind {
            0x02 => out.push((t, true, 0x02, data)),
            0x03 => out.push((t, false, 0x02, data)),
            _ => {}
        }
    }
    Ok(out)
}

/// The ATT operations in HCI traffic, ACL fragments put back together.
fn att_ops(hci: &[Hci]) -> Vec<Att> {
    // Partly received L2CAP frames, per (connection, direction): (time, bytes, total).
    let mut partial: HashMap<(u16, bool), (f64, Vec<u8>, usize)> = HashMap::new();
    let mut frames: Vec<(f64, bool, Vec<u8>)> = Vec::new();
    for (t, sent, kind, data) in hci {
        if *kind != 0x02 || data.len() < 4 {
            continue;
        }
        let hf = u16::from_le_bytes([data[0], data[1]]);
        let (conn, pb) = (hf & 0x0fff, (hf >> 12) & 3);
        let body = &data[4..];
        let key = (conn, *sent);
        if pb == 0b01 {
            if let Some(p) = partial.get_mut(&key) {
                p.1.extend_from_slice(body);
            }
        } else if body.len() >= 4 {
            let total = u16::from_le_bytes([body[0], body[1]]) as usize + 4;
            partial.insert(key, (*t, body.to_vec(), total));
        }
        if let Some(p) = partial.get(&key)
            && p.1.len() >= p.2
        {
            let (t0, frame, total) = partial.remove(&key).unwrap();
            frames.push((t0, *sent, frame[..total].to_vec()));
        }
    }
    let mut out = Vec::new();
    // Long writes in pieces (prepare write), per direction.
    let mut prepared: HashMap<u16, (f64, Vec<u8>)> = HashMap::new();
    // The handle of the last read asked for, per direction of the asker.
    let mut last_read: Option<u16> = None;
    for (t, sent, f) in frames {
        let cid = u16::from_le_bytes([f[2], f[3]]);
        let pdu = &f[4..];
        if cid != 0x0004 || pdu.is_empty() {
            continue;
        }
        let dir = if sent {
            Dir::ToMachine
        } else {
            Dir::FromMachine
        };
        let op = pdu[0];
        let handle = || pdu.get(1..3).map(|h| u16::from_le_bytes([h[0], h[1]]));
        match op {
            // Write request / command / notification / indication: handle, value.
            0x12 | 0x52 | 0x1b | 0x1d => {
                if let Some(h) = handle() {
                    out.push(Att {
                        t,
                        dir,
                        op,
                        handle: h,
                        value: pdu[3..].to_vec(),
                    });
                }
            }
            // Prepare write: handle, offset, part.
            0x16 if pdu.len() >= 5 => {
                let h = u16::from_le_bytes([pdu[1], pdu[2]]);
                prepared
                    .entry(h)
                    .or_insert((t, Vec::new()))
                    .1
                    .extend_from_slice(&pdu[5..]);
            }
            // Execute write: the pieces as one write.
            0x18 => {
                for (h, (t0, v)) in prepared.drain() {
                    out.push(Att {
                        t: t0,
                        dir,
                        op: 0x12,
                        handle: h,
                        value: v,
                    });
                }
            }
            0x0a | 0x0c => last_read = handle(),
            // Read response: what was asked for.
            0x0b => {
                if let Some(h) = last_read {
                    out.push(Att {
                        t,
                        dir,
                        op,
                        handle: h,
                        value: pdu[1..].to_vec(),
                    });
                }
            }
            // Read blob response: more of the last read.
            0x0d => {
                if let Some(last) = out.iter_mut().rev().find(|a| a.op == 0x0b && a.dir == dir) {
                    last.value.extend_from_slice(&pdu[1..]);
                }
            }
            _ => {}
        }
    }
    out
}

/// Every ATT operation in a recording (PacketLogger or btsnoop).
pub fn read_att(bytes: &[u8]) -> Result<Vec<Att>> {
    let hci = if bytes.starts_with(b"btsnoop\0") {
        btsnoop(bytes)?
    } else {
        packetlogger(bytes)?
    };
    Ok(att_ops(&hci))
}

/// The Skitch conversation in a recording: commands written to it and its
/// replies, by name.
pub fn skitch_messages(atts: &[Att]) -> Vec<Message> {
    // The Skitch's write handle: the one most often written with commands
    // it knows; its read handle: the one read most often right after.
    let mut writes: HashMap<u16, usize> = HashMap::new();
    for a in atts {
        if a.dir == Dir::ToMachine
            && matches!(a.op, 0x12 | 0x52)
            && a.value.len() >= 2
            && cmd::name(u16::from_be_bytes([a.value[0], a.value[1]])).is_some()
        {
            *writes.entry(a.handle).or_default() += 1;
        }
    }
    let Some(write) = writes.into_iter().max_by_key(|w| w.1).map(|w| w.0) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for a in atts {
        let from_machine = matches!(a.op, 0x0b | 0x1b | 0x1d) && a.handle != write;
        let to_machine = a.handle == write && matches!(a.op, 0x12 | 0x52);
        if (to_machine || from_machine) && a.value.len() >= 2 {
            out.push(Message {
                t: a.t,
                dir: a.dir,
                command: u16::from_be_bytes([a.value[0], a.value[1]]),
                data: a.value[2..].to_vec(),
            });
        }
    }
    out
}

/// Bytes as hex, with the ones at `marked` in [brackets].
pub fn hex(bytes: &[u8], marked: &[usize]) -> String {
    bytes
        .iter()
        .enumerate()
        .map(|(k, b)| {
            if marked.contains(&k) {
                format!("[{b:02x}]")
            } else {
                format!("{b:02x}")
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Which bytes differ (and any past the shorter one's end).
pub fn changed(a: &[u8], b: &[u8]) -> Vec<usize> {
    (0..a.len().max(b.len()))
        .filter(|&k| a.get(k) != b.get(k))
        .collect()
}

/// A recording as text: each Skitch message on a line, settings replies
/// marked where they differ from the one before. `all`: the upload's data
/// chunks and the status polling too.
pub fn listing(msgs: &[Message], all: bool) -> String {
    let mut out = String::new();
    let mut last_settings: Option<Vec<u8>> = None;
    for m in msgs {
        let noise = matches!(
            m.command,
            cmd::SEND_DATA | cmd::MACHINE_STATE | cmd::PATTERN_SEWING_INFO
        );
        if noise && !all {
            continue;
        }
        let arrow = match m.dir {
            Dir::ToMachine => "->",
            Dir::FromMachine => "<-",
        };
        let marks = if m.command == cmd::MACHINE_SETTING_INFO && m.dir == Dir::FromMachine {
            let marks = last_settings
                .as_ref()
                .map(|l| changed(l, &m.data))
                .unwrap_or_default();
            last_settings = Some(m.data.clone());
            marks
        } else {
            Vec::new()
        };
        let data = if m.data.len() > 64 && !all {
            format!("{} … ({} bytes)", hex(&m.data[..64], &marks), m.data.len())
        } else {
            hex(&m.data, &marks)
        };
        out.push_str(&format!("{:9.3}  {arrow} {:<24} {data}\n", m.t, m.name()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An ATT PDU in an L2CAP frame, split into ACL packets of at most `mtu` bytes.
    fn acl(pdu: &[u8], mtu: usize) -> Vec<Vec<u8>> {
        let mut l2 = (pdu.len() as u16).to_le_bytes().to_vec();
        l2.extend(4u16.to_le_bytes());
        l2.extend(pdu);
        l2.chunks(mtu)
            .enumerate()
            .map(|(k, c)| {
                let pb: u16 = if k == 0 { 0b10 } else { 0b01 };
                let mut p = (0x0040u16 | pb << 12).to_le_bytes().to_vec();
                p.extend((c.len() as u16).to_le_bytes());
                p.extend(c);
                p
            })
            .collect()
    }

    fn write(handle: u16, value: &[u8]) -> Vec<u8> {
        let mut p = vec![0x12];
        p.extend(handle.to_le_bytes());
        p.extend(value);
        p
    }

    /// A short exchange: read the settings, change one, read them again.
    fn exchange() -> Vec<(bool, Vec<u8>)> {
        let settings_a = [0x0c, 0x02, 0x00, 0x05, 0x03, 0x01];
        let settings_b = [0x0c, 0x02, 0x00, 0x06, 0x03, 0x01];
        let mut v = Vec::new();
        for (send, pdu) in [
            (true, write(0x0012, &[0x0c, 0x02])),
            (true, vec![0x0a, 0x10, 0x00]),
            (false, [vec![0x0b], settings_a.to_vec()].concat()),
            (true, write(0x0012, &[0x0c, 0x01, 0x00, 0x06, 0x03, 0x01])),
            (true, write(0x0012, &[0x0c, 0x02])),
            (true, vec![0x0a, 0x10, 0x00]),
            (false, [vec![0x0b], settings_b.to_vec()].concat()),
            // Something else on another handle: not the Skitch's.
            (true, write(0x0030, &[0xff, 0xff, 0x01])),
        ] {
            for p in acl(&pdu, 5) {
                v.push((send, p));
            }
        }
        v
    }

    fn check(msgs: &[Message]) {
        let names: Vec<String> = msgs.iter().map(|m| m.name()).collect();
        assert_eq!(
            names,
            [
                "MACHINE_SETTING_INFO",
                "MACHINE_SETTING_INFO",
                "SET_SETTING_SEND",
                "MACHINE_SETTING_INFO",
                "MACHINE_SETTING_INFO"
            ]
        );
        assert_eq!(msgs[2].data, [0x00, 0x06, 0x03, 0x01]);
        assert_eq!(msgs[1].dir, Dir::FromMachine);
        let text = listing(msgs, false);
        assert!(text.contains("00 [06] 03 01"), "{text}");
    }

    #[test]
    fn reads_an_android_btsnoop_log() {
        let mut f = b"btsnoop\0".to_vec();
        f.extend(1u32.to_be_bytes());
        f.extend(1002u32.to_be_bytes());
        for (k, (send, p)) in exchange().into_iter().enumerate() {
            let data = [vec![0x02], p].concat();
            f.extend((data.len() as u32).to_be_bytes());
            f.extend((data.len() as u32).to_be_bytes());
            f.extend(u32::from(!send).to_be_bytes());
            f.extend(0u32.to_be_bytes());
            f.extend((63_000_000_000_000i64 + k as i64 * 1000).to_be_bytes());
            f.extend(data);
        }
        check(&skitch_messages(&read_att(&f).unwrap()));
    }

    #[test]
    fn reads_an_iphone_packetlogger_trace() {
        let mut f = Vec::new();
        // Some non-ACL noise first (an HCI event).
        for (send, p) in [(None, vec![0x0e, 0x01, 0x00])]
            .into_iter()
            .chain(exchange().into_iter().map(|(s, p)| (Some(s), p)))
        {
            let kind = match send {
                Some(true) => 0x02,
                Some(false) => 0x03,
                None => 0x01,
            };
            f.extend(((p.len() + 9) as u32).to_be_bytes());
            f.extend(1_700_000_000u32.to_be_bytes());
            f.extend(0u32.to_be_bytes());
            f.push(kind);
            f.extend(p);
        }
        check(&skitch_messages(&read_att(&f).unwrap()));
    }
}
