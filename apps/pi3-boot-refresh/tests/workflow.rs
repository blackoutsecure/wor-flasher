mod support;

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::rc::Rc;

use support::*;
use wor_pi3_boot_refresh::workflow::{self, PhysicalDisk, Platform, Volume, VolumeAccess};
use wor_pi3_boot_refresh::{
    BootstrapDisk, DiskLayout, Error, ReadAt, Result, Sector, VolumeExtent, VolumeIdentity,
};

#[derive(Default)]
struct Trace {
    operations: Vec<String>,
    events: Vec<(String, String)>,
    failures: BTreeSet<String>,
    writes: usize,
    backups: Vec<Sector>,
}

impl Trace {
    fn step(&mut self, name: impl Into<String>) -> Result<()> {
        let name = name.into();
        self.operations.push(name.clone());
        if self.failures.contains(&name) {
            Err(Error::io(name, io::Error::other("injected failure")))
        } else {
            Ok(())
        }
    }
}

type Shared = Rc<RefCell<Trace>>;

#[derive(Clone)]
struct Config {
    layout: DiskLayout,
    identities: BTreeMap<String, VolumeIdentity>,
    reopened_identity: Option<VolumeIdentity>,
    initial_windows_disk: u32,
    rechecked_windows_disk: u32,
    boot_extent_disk: u32,
    raw_number: [u8; 12],
    mask: u32,
}

struct FakeVolume {
    drive: String,
    access: VolumeAccess,
    extent_calls: usize,
    config: Rc<RefCell<Config>>,
    trace: Shared,
}

impl FakeVolume {
    fn label(&self) -> String {
        format!(
            "{}:{}",
            self.drive,
            if self.access == VolumeAccess::Read {
                "read"
            } else {
                "rw"
            }
        )
    }
}

impl Volume for FakeVolume {
    fn identity(&mut self, drive: &str) -> Result<VolumeIdentity> {
        assert_eq!(drive, self.drive);
        self.trace
            .borrow_mut()
            .step(format!("identity:{}", self.label()))?;
        let config = self.config.borrow();
        if self.access == VolumeAccess::ReadWrite
            && let Some(identity) = &config.reopened_identity
        {
            return Ok(identity.clone());
        }
        Ok(config.identities[drive].clone())
    }

    fn extent(&mut self) -> Result<VolumeExtent> {
        self.extent_calls += 1;
        self.trace
            .borrow_mut()
            .step(format!("extent:{}:{}", self.drive, self.extent_calls))?;
        let config = self.config.borrow();
        let mut extent = if self.drive == "W:" {
            config.layout.windows
        } else {
            config.layout.boot
        };
        extent.disk_index = if self.drive == "W:" {
            if self.extent_calls == 1 {
                config.initial_windows_disk
            } else {
                config.rechecked_windows_disk
            }
        } else {
            config.boot_extent_disk
        };
        Ok(extent)
    }

    fn sync(&mut self) -> Result<()> {
        self.trace.borrow_mut().step("flush-boot")
    }

    fn close(&mut self) -> Result<()> {
        self.trace
            .borrow_mut()
            .step(format!("close:{}", self.label()))
    }
}

struct FakeDisk {
    inner: MemoryDisk,
    config: Rc<RefCell<Config>>,
    trace: Shared,
}

impl ReadAt for FakeDisk {
    fn read_at(&mut self, data: &mut [u8], offset: u64) -> io::Result<usize> {
        self.trace
            .borrow_mut()
            .operations
            .push(format!("read-sector:{}", offset / 512));
        self.inner.read_at(data, offset)
    }
}

impl BootstrapDisk for FakeDisk {
    fn write_at(&mut self, data: &[u8], offset: u64) -> io::Result<usize> {
        let mut trace = self.trace.borrow_mut();
        trace.operations.push("write-sector:0".into());
        assert_eq!(
            trace.backups.len(),
            1,
            "write happened without a completed backup"
        );
        trace.writes += 1;
        drop(trace);
        self.inner.write_at(data, offset)
    }

    fn sync(&mut self) -> io::Result<()> {
        self.trace.borrow_mut().operations.push("flush-disk".into());
        self.inner.sync()
    }
}

impl PhysicalDisk for FakeDisk {
    fn device_number(&mut self) -> Result<[u8; 12]> {
        self.trace.borrow_mut().step("number")?;
        Ok(self.config.borrow().raw_number)
    }

    fn geometry(&mut self) -> Result<[u8; 24]> {
        self.trace.borrow_mut().step("geometry")?;
        let mut geometry = [0; 24];
        put32(
            &mut geometry,
            20,
            self.config.borrow().layout.bytes_per_sector,
        );
        Ok(geometry)
    }

    fn length(&mut self) -> Result<[u8; 8]> {
        self.trace.borrow_mut().step("length")?;
        Ok(self.config.borrow().layout.bytes.to_le_bytes())
    }

    fn close(&mut self) -> Result<()> {
        self.trace.borrow_mut().step("close:disk")
    }
}

struct FakePlatform {
    disk: Option<MemoryDisk>,
    config: Rc<RefCell<Config>>,
    trace: Shared,
}

impl FakePlatform {
    fn new() -> Self {
        let (disk, layout) = fixture(Geometry::default());
        let mut raw_number = [0; 12];
        put32(&mut raw_number, 0, 7);
        put32(&mut raw_number, 4, layout.disk_index);
        let identities = [("B:", 7, 7, 1), ("W:", 7, 7, 3)]
            .into_iter()
            .map(|(drive, device_type, disk_index, partition_index)| {
                (
                    drive.into(),
                    VolumeIdentity {
                        drive: drive.into(),
                        device_type,
                        disk_index,
                        partition_index,
                    },
                )
            })
            .collect();
        Self {
            disk: Some(disk),
            config: Rc::new(RefCell::new(Config {
                layout,
                identities,
                reopened_identity: None,
                initial_windows_disk: 7,
                rechecked_windows_disk: 7,
                boot_extent_disk: 7,
                raw_number,
                mask: (1 << 1) | (1 << 22),
            })),
            trace: Rc::new(RefCell::new(Trace::default())),
        }
    }

    fn run(&mut self, duplicate: bool) -> Result<&'static str> {
        workflow::run(self, |key| {
            match key {
                "WOR_DEVICE_TYPE" => "RPi3-ARM64",
                "WOR_INSTALLOPTIONS_PARTITIONSCHEME" => "GPT",
                "WOR_IMAGE_ARCH" => "ARM64",
                "WOR_DISK_INDEX" => "7",
                "WOR_DISK_BOOTPARTITION" => {
                    if duplicate {
                        "W:"
                    } else {
                        "B:"
                    }
                }
                "WOR_DISK_WINDOWSPARTITION" => "W:",
                _ => "",
            }
            .into()
        })
    }
}

impl Platform for FakePlatform {
    type Volume = FakeVolume;
    type Disk = FakeDisk;

    fn require_windows_pe(&mut self) -> Result<()> {
        self.trace.borrow_mut().step("winpe")
    }

    fn require_file(&mut self, path: &str) -> Result<()> {
        self.trace.borrow_mut().step(format!("file:{path}"))
    }

    fn logical_drives(&mut self) -> Result<u32> {
        self.trace.borrow_mut().step("logical-drives")?;
        Ok(self.config.borrow().mask)
    }

    fn open_volume(&mut self, drive: &str, access: VolumeAccess) -> Result<Self::Volume> {
        self.trace.borrow_mut().step(format!(
            "open-volume:{drive}:{}",
            if access == VolumeAccess::Read {
                "read"
            } else {
                "rw"
            }
        ))?;
        if !self.config.borrow().identities.contains_key(drive) {
            return Err(Error::io(
                "unavailable fixture drive",
                io::Error::from(io::ErrorKind::NotFound),
            ));
        }
        Ok(FakeVolume {
            drive: drive.into(),
            access,
            extent_calls: 0,
            config: self.config.clone(),
            trace: self.trace.clone(),
        })
    }

    fn open_disk(&mut self, index: u32) -> Result<Self::Disk> {
        self.trace.borrow_mut().step(format!("open-disk:{index}"))?;
        assert_eq!(index, 7, "an unselected disk was opened");
        Ok(FakeDisk {
            inner: self.disk.take().unwrap(),
            config: self.config.clone(),
            trace: self.trace.clone(),
        })
    }

    fn save_backup(&mut self, directory: &str, sector: &Sector) -> Result<()> {
        assert_eq!(directory, r"W:\Windows\Logs\WoR-Flasher");
        let mut trace = self.trace.borrow_mut();
        trace.step("backup")?;
        trace.backups.push(*sector);
        Ok(())
    }

    fn event(&mut self, level: &str, message: &str) {
        self.trace
            .borrow_mut()
            .events
            .push((level.into(), message.into()));
    }
}

#[test]
fn windows_flow_validates_installed_files_and_binds_every_open_before_sector_zero() {
    let mut platform = FakePlatform::new();
    assert_eq!(
        platform.run(false).unwrap(),
        "Pi 3 installed boot-sector repair verified; GPT and FAT metadata are unchanged."
    );
    let trace = platform.trace.borrow();
    assert_eq!(trace.writes, 1);
    let expected = [
        "winpe",
        "open-volume:W::read",
        "extent:W::1",
        r"file:W:\Windows\System32\ntoskrnl.exe",
        "open-volume:B::read",
        "identity:B::read",
        "close:B::read",
        r"file:B:\RPI_EFI.fd",
        "open-volume:B::rw",
        "identity:B::rw",
        "extent:B::1",
        "extent:W::2",
        "open-disk:7",
        "number",
        "geometry",
        "length",
        "flush-boot",
    ];
    assert_eq!(trace.operations[..expected.len()], expected);
    assert!(
        !trace
            .operations
            .iter()
            .any(|value| value == "logical-drives")
    );
    let backup = trace
        .operations
        .iter()
        .position(|value| value == "backup")
        .unwrap();
    let write = trace
        .operations
        .iter()
        .position(|value| value == "write-sector:0")
        .unwrap();
    assert!(backup < write);
    assert!(
        trace.operations[backup + 1..write]
            .iter()
            .any(|value| value == "read-sector:1")
    );
    assert_eq!(
        trace.operations[trace.operations.len() - 3..],
        ["close:disk", "close:B::rw", "close:W::read"]
    );
    assert_eq!(trace.events[0].0, "LogInfo");
    assert!(trace.events[0].1.contains("boot B:, Windows W:, disk 7"));
}

#[test]
fn duplicate_exports_enumerate_mounted_drives_and_only_tolerate_individual_probe_errors() {
    let mut platform = FakePlatform::new();
    platform.config.borrow_mut().mask |= 1;
    platform.run(true).unwrap();
    let trace = platform.trace.borrow();
    assert_eq!(trace.writes, 1);
    assert!(
        trace
            .operations
            .iter()
            .any(|value| value == "logical-drives")
    );
    assert!(trace.events.iter().any(|(level, message)| level == "LogWarn" && message.contains("both boot and Windows")));
    assert!(
        trace
            .events
            .iter()
            .any(|(level, message)| level == "LogDebug" && message.contains("A:"))
    );
    drop(trace);
    let mut explicit = FakePlatform::new();
    explicit
        .trace
        .borrow_mut()
        .failures
        .insert("open-volume:B::read".into());
    assert!(explicit.run(false).is_err());
    assert!(
        !explicit
            .trace
            .borrow()
            .operations
            .iter()
            .any(|value| value == "logical-drives")
    );
    assert_eq!(explicit.trace.borrow().writes, 0);
}

#[test]
fn every_prewrite_os_failure_denies_write_and_closes_all_opened_devices() {
    for failure in [
        "winpe",
        "open-volume:W::read",
        "extent:W::1",
        r"file:W:\Windows\System32\ntoskrnl.exe",
        "open-volume:B::read",
        "identity:B::read",
        "close:B::read",
        r"file:B:\RPI_EFI.fd",
        "open-volume:B::rw",
        "identity:B::rw",
        "extent:B::1",
        "extent:W::2",
        "open-disk:7",
        "number",
        "geometry",
        "length",
        "flush-boot",
        "backup",
    ] {
        let mut platform = FakePlatform::new();
        platform.trace.borrow_mut().failures.insert(failure.into());
        let error = platform.run(false).unwrap_err();
        assert!(
            error.to_string().contains("injected failure"),
            "{failure}: {error}"
        );
        let trace = platform.trace.borrow();
        assert_eq!(trace.writes, 0, "{failure}");
        for (opened, closed) in [
            ("open-volume:W::read", "close:W::read"),
            ("open-volume:B::rw", "close:B::rw"),
            ("open-disk:7", "close:disk"),
        ] {
            if failure != opened && trace.operations.iter().any(|value| value == opened) {
                assert!(
                    trace.operations.iter().any(|value| value == closed),
                    "{failure}: missing {closed}"
                );
            }
        }
    }
}

#[test]
fn boot_identity_extent_rebinding_and_raw_device_aliases_fail_closed() {
    for name in [
        "windows-initial-disk",
        "windows-rechecked-disk",
        "boot-extent-disk",
        "reopened-boot",
        "raw-disk",
        "raw-partition",
        "sector-size",
        "aliased-extents",
        "raw-length",
    ] {
        let mut platform = FakePlatform::new();
        {
            let mut config = platform.config.borrow_mut();
            match name {
                "windows-initial-disk" => config.initial_windows_disk = 8,
                "windows-rechecked-disk" => config.rechecked_windows_disk = 8,
                "boot-extent-disk" => config.boot_extent_disk = 8,
                "reopened-boot" => {
                    config.reopened_identity = Some(VolumeIdentity {
                        drive: "B:".into(),
                        device_type: 7,
                        disk_index: 7,
                        partition_index: 2,
                    })
                }
                "raw-disk" => put32(&mut config.raw_number, 4, 8),
                "raw-partition" => put32(&mut config.raw_number, 8, 1),
                "sector-size" => config.layout.bytes_per_sector = 4096,
                "aliased-extents" => config.layout.windows = config.layout.boot,
                "raw-length" => config.layout.bytes += 512,
                _ => unreachable!(),
            }
        }
        assert!(platform.run(false).is_err(), "{name}");
        assert_eq!(platform.trace.borrow().writes, 0, "{name}");
    }
}

#[test]
fn empty_or_failed_enumeration_is_fatal_and_close_errors_cannot_report_success() {
    for failure in [
        "logical-drives",
        "empty-mask",
        "close:disk",
        "close:B::rw",
        "close:W::read",
    ] {
        let mut platform = FakePlatform::new();
        if failure == "empty-mask" {
            platform.config.borrow_mut().mask = 0;
        } else {
            platform.trace.borrow_mut().failures.insert(failure.into());
        }
        assert!(
            platform
                .run(failure == "logical-drives" || failure == "empty-mask")
                .is_err(),
            "{failure}"
        );
    }
    let mut platform = FakePlatform::new();
    platform.trace.borrow_mut().failures.extend([
        "backup".into(),
        "close:disk".into(),
        "close:B::rw".into(),
        "close:W::read".into(),
    ]);
    let error = platform.run(false).unwrap_err().to_string();
    for cause in ["backup", "close:disk", "close:B::rw", "close:W::read"] {
        assert!(error.contains(cause), "missing {cause}: {error}");
    }
    assert_eq!(platform.trace.borrow().writes, 0);
}

#[test]
fn unrelated_board_and_mbr_are_successful_not_applicable_without_any_os_access() {
    for (board, scheme) in [
        ("RPi4-ARM64", ""),
        ("RPi5-ARM64", ""),
        ("RPi3-ARM64", "MBR"),
    ] {
        let mut platform = FakePlatform::new();
        let message = workflow::run(&mut platform, |key| {
            match key {
                "WOR_DEVICE_TYPE" => board,
                "WOR_INSTALLOPTIONS_PARTITIONSCHEME" => scheme,
                _ => "",
            }
            .into()
        })
        .unwrap();
        assert!(message.contains("no disk accessed"));
        assert!(platform.trace.borrow().operations.is_empty());
    }
    let mut platform = FakePlatform::new();
    assert!(workflow::run(&mut platform, |_| String::new()).is_err());
    assert!(platform.trace.borrow().operations.is_empty());
}

#[test]
fn matching_bootstrap_returns_read_only_message_without_backup() {
    let mut platform = FakePlatform::new();
    let layout = platform.config.borrow().layout;
    let plan =
        wor_pi3_boot_refresh::build_bootstrap(platform.disk.as_mut().unwrap(), &layout).unwrap();
    platform
        .disk
        .as_ref()
        .unwrap()
        .image
        .borrow_mut()
        .insert(0, plan.replacement);
    assert!(platform.run(false).unwrap().contains("already matches"));
    assert_eq!(platform.trace.borrow().writes, 0);
    assert!(platform.trace.borrow().backups.is_empty());
}
