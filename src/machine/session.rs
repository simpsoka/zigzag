//! A conversation with a Skitch over any link: Bluetooth (`skitch.rs`) or
//! the simulated machine (`sim.rs`). Blocking calls, made from a worker
//! thread; the command sequences follow Artspira's, as Respira's
//! BrotherPP1Service does (https://github.com/jhbruhn/respira, Apache-2.0).

use super::protocol::{self, Layout, MachineInfo, PatternInfo, Progress, Status, cmd};
use crate::formats::pen::PenData;
use anyhow::{Result, anyhow, bail};
use std::time::Duration;

/// Sends commands and reads replies.
pub trait Transport: Send {
    /// Write a framed command and read the reply (`[cmd_hi, cmd_lo, …]`).
    fn request(&mut self, command: u16, payload: &[u8]) -> Result<Vec<u8>>;
    /// Write a framed command without reading a reply (the upload's chunks).
    fn send(&mut self, command: u16, payload: &[u8]) -> Result<()>;
    /// Read the reply characteristic again, without writing (for a reply
    /// that wasn't ready the first time).
    fn read(&mut self) -> Result<Vec<u8>> {
        Ok(Vec::new())
    }
    /// Still connected.
    fn connected(&mut self) -> bool;
    fn close(&mut self);
    /// How long to let the machine take in streamed chunks.
    fn settle(&self) -> Duration {
        Duration::from_millis(500)
    }
}

/// The machine answered with nothing: it isn't paired with this computer.
#[derive(Debug)]
pub struct NotPaired;

impl std::fmt::Display for NotPaired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("The machine didn't answer, so it isn't paired with this computer yet.")
    }
}

impl std::error::Error for NotPaired {}

/// No machine turned up while scanning.
#[derive(Debug)]
pub struct NotFound;

impl std::fmt::Display for NotFound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("No Skitch found nearby.")
    }
}

impl std::error::Error for NotFound {}

pub struct Session {
    link: Box<dyn Transport>,
}

impl Session {
    /// Start talking over `link`, checking the machine answers (an
    /// unpaired one connects but replies with nothing).
    pub fn open(link: Box<dyn Transport>) -> Result<Session> {
        let mut s = Session { link };
        match s.link.request(cmd::MACHINE_STATE, &[]) {
            Ok(r) if r.len() >= 3 => Ok(s),
            Ok(_) => {
                s.link.close();
                Err(NotPaired.into())
            }
            Err(e) => {
                s.link.close();
                Err(e.context(NotPaired))
            }
        }
    }

    /// A command whose reply we read: it must have one. A machine still
    /// busy (just after an upload, say) can answer late, so read again
    /// before giving up.
    fn request(&mut self, command: u16, payload: &[u8]) -> Result<Vec<u8>> {
        let mut r = self.link.request(command, payload)?;
        for wait in [150, 400] {
            if r.len() >= 3 {
                return Ok(r);
            }
            std::thread::sleep(Duration::from_millis(wait));
            r = self.link.read()?;
        }
        if r.len() >= 3 {
            return Ok(r);
        }
        bail!(
            "the machine sent an empty reply to {} ({} bytes)",
            cmd::name(command)
                .map(String::from)
                .unwrap_or_else(|| format!("0x{command:04x}")),
            r.len()
        )
    }

    /// A command whose reply we don't need: a short one is fine (as
    /// Artspira and Respira treat them; the delete and the layout can
    /// answer with less than a status byte).
    fn tell(&mut self, command: u16, payload: &[u8]) -> Result<()> {
        self.link.request(command, payload)?;
        Ok(())
    }

    pub fn connected(&mut self) -> bool {
        self.link.connected()
    }

    pub fn close(&mut self) {
        self.link.close();
    }

    pub fn info(&mut self) -> Result<MachineInfo> {
        let r = self.request(cmd::MACHINE_INFO, &[])?;
        protocol::parse_machine_info(&r)
            .ok_or_else(|| anyhow!("short MACHINE_INFO reply ({} bytes)", r.len()))
    }

    /// Stitches since the last service, and ever.
    pub fn service_count(&mut self) -> Result<(u32, u32)> {
        let r = self.request(cmd::SERVICE_COUNT, &[])?;
        protocol::parse_service_count(&r).ok_or_else(|| anyhow!("short SERVICE_COUNT reply"))
    }

    /// What it's doing, and its error byte ([`protocol::NO_ERROR`] if none).
    pub fn state(&mut self) -> Result<(Status, u8)> {
        let r = self.request(cmd::MACHINE_STATE, &[])?;
        Ok((
            Status::from_byte(r[2]),
            r.get(4).copied().unwrap_or(protocol::NO_ERROR),
        ))
    }

    pub fn progress(&mut self) -> Result<Progress> {
        let r = self.request(cmd::PATTERN_SEWING_INFO, &[])?;
        protocol::parse_progress(&r).ok_or_else(|| anyhow!("short progress reply"))
    }

    pub fn pattern_info(&mut self) -> Result<PatternInfo> {
        let r = self.request(cmd::EMB_SEWING_INFO_REQUEST, &[])?;
        protocol::parse_pattern_info(&r).ok_or_else(|| anyhow!("short pattern info reply"))
    }

    /// The id of the pattern on the machine (`None`: no pattern).
    pub fn pattern_uuid(&mut self) -> Result<Option<[u8; 16]>> {
        let r = self.request(cmd::PATTERN_UUID_REQUEST, &[])?;
        let Some(id) = r.get(2..18) else {
            return Ok(None);
        };
        let id: [u8; 16] = id.try_into()?;
        Ok((id != [0; 16]).then_some(id))
    }

    pub fn delete_pattern(&mut self) -> Result<()> {
        self.tell(cmd::EMB_SEWING_DATA_DELETE, &[])
    }

    /// Upload a pattern, placed by `layout` (see [`Layout::centered`] and
    /// [`Layout::as_placed`]), under the id `uuid`.
    pub fn upload(
        &mut self,
        pen: &PenData,
        layout: Layout,
        uuid: [u8; 16],
        mut on_progress: impl FnMut(f32),
    ) -> Result<()> {
        self.delete_pattern()?;
        let r = self.request(cmd::SEND_DATA_INFO, &protocol::data_info(&pen.bytes))?;
        if r[2] != 0 {
            bail!(
                "the machine turned the design down (size check 0x{:02x})",
                r[2]
            );
        }
        let chunks = protocol::data_chunks(&pen.bytes);
        let total = chunks.len().max(1);
        for (i, chunk) in chunks.iter().enumerate() {
            self.link.send(cmd::SEND_DATA, chunk)?;
            on_progress((i + 1) as f32 / total as f32);
        }
        std::thread::sleep(self.link.settle());
        self.tell(cmd::LAYOUT_SEND, &layout.payload())?;
        let r = self.request(cmd::EMB_UUID_SEND, &uuid)?;
        if r[2] != 0 {
            bail!("the machine turned the design down (0x{:02x})", r[2]);
        }
        Ok(())
    }

    /// Trace the design's outline with the hoop (after the machine's Accept).
    pub fn mask_trace(&mut self) -> Result<()> {
        self.tell(cmd::MACHINE_SETTING_INFO, &[])?;
        self.tell(cmd::MASK_TRACE, &[0x00])
    }

    /// Ready the machine to sew (or to sew on): its Start button then starts it.
    pub fn start_sewing(&mut self) -> Result<()> {
        self.tell(cmd::START_SEWING, &[])
    }

    /// The machine's settings, as it reports them (the reply after its
    /// command id). What each byte means isn't known yet (see `capture.rs`).
    pub fn settings(&mut self) -> Result<Vec<u8>> {
        let r = self.request(cmd::MACHINE_SETTING_INFO, &[])?;
        Ok(r[2..].to_vec())
    }

    /// Move to stitch `index` (while paused), to sew on from there.
    pub fn set_stitch(&mut self, index: u16) -> Result<()> {
        self.tell(cmd::NEEDLE_MODE_INSTRUCTIONS, &index.to_le_bytes())
    }
}
