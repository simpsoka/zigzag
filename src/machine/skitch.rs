//! Bluetooth LE client for the Brother PP1 ("Skitch").
//!
//! The machine must be paired with the computer first: long-press the
//! Bluetooth button on the machine until it beeps, then connect. (It's a
//! Bluetooth LE device: on a Mac it never shows in the Bluetooth settings,
//! and pairs when an app connects.) An unpaired machine accepts the
//! connection but returns empty replies.
//!
//! Upload sequence (same as Artspira, per Respira): delete the current pattern,
//! announce size + checksum, stream 500-byte chunks without waiting for replies,
//! send the layout (position/frame), then a random 16-byte pattern UUID.

use super::protocol::{self, Layout, MachineInfo, Progress, Status, cmd};
use crate::formats::pen::PenData;
use anyhow::{Context, Result, anyhow, bail};
use btleplug::api::{
    Central, Characteristic, Manager as _, Peripheral as _, ScanFilter, WriteType,
};
use btleplug::platform::{Manager, Peripheral};
use std::time::Duration;
use tokio::time::sleep;
use uuid::Uuid;

pub struct Skitch {
    periph: Peripheral,
    write: Characteristic,
    read: Characteristic,
}

const NOT_PAIRED: &str = "The machine didn't answer. It's probably not paired: hold the Bluetooth \
button on the Skitch until it beeps, then try again straight away.";

impl Skitch {
    /// Scan for a Skitch and connect. `name` filters by advertised name (substring).
    pub async fn connect(scan_for: Duration, name: Option<&str>) -> Result<Skitch> {
        let skitch = Self::connect_unchecked(scan_for, name).await?;
        // Validate: an unpaired machine replies with nothing.
        skitch.state().await.context(NOT_PAIRED)?;
        Ok(skitch)
    }

    /// [`Skitch::connect`] without checking it answers (a [`Session`] does).
    ///
    /// [`Session`]: super::session::Session
    pub async fn connect_unchecked(scan_for: Duration, name: Option<&str>) -> Result<Skitch> {
        let service = Uuid::parse_str(protocol::SERVICE_UUID)?;
        let manager = Manager::new().await.context("starting Bluetooth")?;
        let adapter = manager
            .adapters()
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("no Bluetooth adapter found"))?;
        adapter
            .start_scan(ScanFilter {
                services: vec![service],
            })
            .await?;

        let deadline = tokio::time::Instant::now() + scan_for;
        let periph = loop {
            let mut found = None;
            for p in adapter.peripherals().await? {
                let Some(props) = p.properties().await? else {
                    continue;
                };
                let has_service = props.services.contains(&service);
                let name_ok = match (name, &props.local_name) {
                    (Some(want), Some(n)) => n.to_lowercase().contains(&want.to_lowercase()),
                    (Some(_), None) => false,
                    (None, _) => true,
                };
                if has_service && name_ok {
                    found = Some(p);
                    break;
                }
            }
            if let Some(p) = found {
                break p;
            }
            if tokio::time::Instant::now() >= deadline {
                let _ = adapter.stop_scan().await;
                return Err(anyhow::Error::new(super::session::NotFound).context(
                    "no Skitch found. Is it on, in range, and not connected to the Artspira app?",
                ));
            }
            sleep(Duration::from_millis(300)).await;
        };
        let _ = adapter.stop_scan().await;

        periph.connect().await.context("connecting to the Skitch")?;
        periph.discover_services().await?;
        let find = |id: &str| -> Result<Characteristic> {
            let id = Uuid::parse_str(id)?;
            periph
                .characteristics()
                .into_iter()
                .find(|c| c.uuid == id)
                .ok_or_else(|| anyhow!("characteristic {id} missing; is this a Brother PP1?"))
        };
        Ok(Skitch {
            write: find(protocol::WRITE_CHAR_UUID)?,
            read: find(protocol::READ_CHAR_UUID)?,
            periph,
        })
    }

    /// The raw reply to a command (empty from an unpaired machine).
    async fn raw(&self, command: u16, payload: &[u8]) -> Result<Vec<u8>> {
        self.periph
            .write(
                &self.write,
                &protocol::frame(command, payload),
                WriteType::WithResponse,
            )
            .await?;
        sleep(Duration::from_millis(50)).await;
        Ok(self.periph.read(&self.read).await?)
    }

    /// Read the reply characteristic again, without writing.
    async fn reread(&self) -> Result<Vec<u8>> {
        Ok(self.periph.read(&self.read).await?)
    }

    pub async fn disconnect(&self) -> Result<()> {
        self.periph.disconnect().await?;
        Ok(())
    }

    /// Send a command and read its reply.
    pub async fn request(&self, command: u16, payload: &[u8]) -> Result<Vec<u8>> {
        self.periph
            .write(
                &self.write,
                &protocol::frame(command, payload),
                WriteType::WithResponse,
            )
            .await?;
        sleep(Duration::from_millis(50)).await;
        let resp = self.periph.read(&self.read).await?;
        if resp.len() < 3 {
            bail!(NOT_PAIRED);
        }
        Ok(resp)
    }

    async fn send(&self, command: u16, payload: &[u8]) -> Result<()> {
        self.periph
            .write(
                &self.write,
                &protocol::frame(command, payload),
                WriteType::WithResponse,
            )
            .await?;
        sleep(Duration::from_millis(10)).await;
        Ok(())
    }

    pub async fn info(&self) -> Result<MachineInfo> {
        let r = self.request(cmd::MACHINE_INFO, &[]).await?;
        protocol::parse_machine_info(&r)
            .ok_or_else(|| anyhow!("short MACHINE_INFO reply ({} bytes)", r.len()))
    }

    /// Current status and error code.
    pub async fn state(&self) -> Result<(Status, u8)> {
        let r = self.request(cmd::MACHINE_STATE, &[]).await?;
        Ok((Status::from_byte(r[2]), r.get(4).copied().unwrap_or(0)))
    }

    pub async fn progress(&self) -> Result<Progress> {
        let r = self.request(cmd::PATTERN_SEWING_INFO, &[]).await?;
        protocol::parse_progress(&r).ok_or_else(|| anyhow!("short progress reply"))
    }

    /// Upload a pattern, centered in the hoop. Returns the pattern UUID.
    pub async fn upload(
        &self,
        pen: &PenData,
        mut on_progress: impl FnMut(f32),
    ) -> Result<[u8; 16]> {
        self.request(cmd::EMB_SEWING_DATA_DELETE, &[]).await?;
        let r = self
            .request(cmd::SEND_DATA_INFO, &protocol::data_info(&pen.bytes))
            .await?;
        if r[2] != 0 {
            bail!(
                "machine rejected the pattern size/checksum (status 0x{:02x})",
                r[2]
            );
        }
        let chunks = protocol::data_chunks(&pen.bytes);
        let total = chunks.len();
        for (i, chunk) in chunks.iter().enumerate() {
            self.send(cmd::SEND_DATA, chunk).await?;
            on_progress((i + 1) as f32 / total as f32);
        }
        sleep(Duration::from_millis(500)).await;

        let layout = Layout::centered(pen.bounds);
        self.request(cmd::LAYOUT_SEND, &layout.payload()).await?;

        let uuid = *Uuid::new_v4().as_bytes();
        let r = self.request(cmd::EMB_UUID_SEND, &uuid).await?;
        if r[2] != 0 {
            bail!("machine rejected the pattern (status 0x{:02x})", r[2]);
        }
        Ok(uuid)
    }

    /// Trace the design outline with the hoop so you can check placement.
    pub async fn mask_trace(&self) -> Result<()> {
        self.request(cmd::MACHINE_SETTING_INFO, &[]).await?;
        self.request(cmd::MASK_TRACE, &[0x00]).await?;
        Ok(())
    }

    /// The machine's settings reply (after its command id).
    pub async fn settings(&self) -> Result<Vec<u8>> {
        let r = self.request(cmd::MACHINE_SETTING_INFO, &[]).await?;
        Ok(r[2..].to_vec())
    }

    pub async fn start_sewing(&self) -> Result<()> {
        self.request(cmd::START_SEWING, &[]).await?;
        Ok(())
    }
}

/// A Skitch over Bluetooth as a blocking [`Transport`], for a worker thread.
pub struct BleLink {
    rt: tokio::runtime::Runtime,
    skitch: Skitch,
}

impl BleLink {
    /// Scan for a Skitch (up to `scan_for`) and connect to it.
    pub fn connect(scan_for: Duration) -> Result<BleLink> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .context("starting Bluetooth")?;
        let skitch = rt.block_on(Skitch::connect_unchecked(scan_for, None))?;
        Ok(BleLink { rt, skitch })
    }
}

impl super::session::Transport for BleLink {
    fn request(&mut self, command: u16, payload: &[u8]) -> Result<Vec<u8>> {
        self.rt.block_on(self.skitch.raw(command, payload))
    }

    fn send(&mut self, command: u16, payload: &[u8]) -> Result<()> {
        self.rt.block_on(self.skitch.send(command, payload))
    }

    fn read(&mut self) -> Result<Vec<u8>> {
        self.rt.block_on(self.skitch.reread())
    }

    fn connected(&mut self) -> bool {
        self.rt
            .block_on(self.skitch.periph.is_connected())
            .unwrap_or(false)
    }

    fn close(&mut self) {
        let _ = self.rt.block_on(self.skitch.disconnect());
    }
}
