mod support;

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::process::Command;

use support::{FixtureDirectory, repo_root};
use wor_pi3_boot_refresh::{
    BootstrapDisk, DiskLayout, ReadAt, VolumeExtent, build_bootstrap, refresh_bootstrap,
};

const CREATE: &str = r#"
import hashlib, os, runpy, struct, sys
from pathlib import Path
root, path = Path(sys.argv[1]), Path(sys.argv[2])
start, count, reserved, cluster, fat, root_cluster = map(int, sys.argv[3:9])
installed = sys.argv[9] == "installed"
ns = runpy.run_path(str(root / "tests/test-pi3-gpt-bootstrap.py"))
helper = runpy.run_path(str(root / "src/lib/pi3-hybrid-mbr.py"))
stale = None
if installed:
    ns["create_gpt_image"](path, sector_count=3145728, sectors_per_cluster=8, fat_sectors=4096)
    old_size = path.stat().st_size
    handle = os.open(path, os.O_RDONLY)
    try:
        stale = helper["build_mbr"](handle)
    finally:
        os.close(handle)
ns["create_gpt_image"](path, start, count, reserved, cluster, fat, root_cluster,
    ns["BASIC_DATA_TYPE"] if installed else ns["EFI_TYPE"])
with path.open("r+b") as disk:
    if installed:
        disk.truncate(old_size)
        disk.write(stale)
    disk.seek(512)
    header = bytearray(disk.read(512))
    entries = bytearray(disk.read(16384))
    if installed:
        backup_lba = old_size // 512 - 1
        boot_end = struct.unpack_from("<Q", entries, 40)[0]
        windows_start, temporary_start = boot_end + 1 + 32768, backup_lba - 32768
        entries[384:512] = entries[128:256]
        entries[128:384] = bytes(256)
        entries[128:144] = bytes.fromhex("16e3c9e35c0bb84d817df92df00215ae")
        entries[144:160] = b"\x44" * 16
        struct.pack_into("<QQ", entries, 160, boot_end + 1, windows_start - 1)
        entries[256:272] = ns["BASIC_DATA_TYPE"]
        entries[272:288] = b"\x55" * 16
        struct.pack_into("<QQ", entries, 288, temporary_start, backup_lba - 33)
        struct.pack_into("<QQ", entries, 416, windows_start, temporary_start - 1)
        struct.pack_into("<Q", header, 32, backup_lba)
        struct.pack_into("<Q", header, 48, backup_lba - 33)
        struct.pack_into("<I", header, 88, ns["zlib"].crc32(entries))
        ns["update_header_checksum"](header)
        disk.seek(512)
        disk.write(header)
        disk.write(entries)
        backup = bytearray(header)
        struct.pack_into("<QQ", backup, 24, backup_lba, 1)
        struct.pack_into("<Q", backup, 72, backup_lba - 32)
        ns["update_header_checksum"](backup)
        disk.seek((backup_lba - 32) * 512)
        disk.write(entries)
        disk.write(backup)
    boot_start, boot_end = struct.unpack_from("<QQ", entries, 32)
    win_slot = 384 if installed else 128
    win_start, win_end = struct.unpack_from("<QQ", entries, win_slot + 32)
    disk.seek(boot_start * 512)
    boot = disk.read(512)
handle = os.open(path, os.O_RDONLY)
try:
    expected = helper["build_mbr"](handle)
finally:
    os.close(handle)
for value in [path.stat().st_size, boot_start * 512, (boot_end - boot_start + 1) * 512,
              win_start * 512, (win_end - win_start + 1) * 512]:
    print(value)
print(expected.hex())
print(hashlib.sha256(header + entries + boot).hexdigest())
"#;

const VERIFY: &str = r#"
import runpy, sys
from pathlib import Path
root, path, start = Path(sys.argv[1]), Path(sys.argv[2]), int(sys.argv[3])
ns = runpy.run_path(str(root / "tests/test-pi3-gpt-bootstrap.py"))
assert ns["read_firmware_file"](path, 0) == ns["read_firmware_file"](path, start)
sys.argv = ["pi3-hybrid-mbr.py", "--check", str(path)]
runpy.run_path(str(root / "src/lib/pi3-hybrid-mbr.py"), run_name="__main__")
"#;

fn python(script: &str, args: &[String]) -> String {
    let result = Command::new("python3")
        .args(["-B", "-c", script])
        .args(args)
        .output()
        .expect("source-oracle tests require python3, not Go or physical media");
    assert!(
        result.status.success(),
        "Python fixture failed:\n{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap()
}

fn hex<const N: usize>(value: &str) -> [u8; N] {
    assert_eq!(value.len(), N * 2);
    std::array::from_fn(|index| u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).unwrap())
}

struct FixtureFile {
    file: File,
    writes: usize,
}

impl FixtureFile {
    fn open(path: &Path) -> Self {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .unwrap();
        assert!(
            file.metadata().unwrap().is_file(),
            "oracle accepts regular synthetic files only"
        );
        Self { file, writes: 0 }
    }
}

impl ReadAt for FixtureFile {
    fn read_at(&mut self, data: &mut [u8], offset: u64) -> io::Result<usize> {
        assert_eq!(data.len(), 512);
        assert_eq!(offset % 512, 0);
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.read(data)
    }
}

impl BootstrapDisk for FixtureFile {
    fn write_at(&mut self, data: &[u8], offset: u64) -> io::Result<usize> {
        assert_eq!(data.len(), 512);
        assert_eq!(offset, 0);
        self.writes += 1;
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.write(data)
    }

    fn sync(&mut self) -> io::Result<()> {
        self.file.sync_all()
    }
}

#[test]
fn native_bootstrap_and_metadata_digest_match_desktop_oracle_and_fragmented_firmware() {
    for (name, geometry) in [
        ("default", [2048_u64, 131072, 32, 1, 1024, 2]),
        ("installed-128-mib", [2048, 262144, 32, 1, 2048, 2]),
        ("installer-1536-mib", [2048, 3145728, 32, 8, 4096, 2]),
        ("alternate-offset", [4096, 131072, 64, 1, 1024, 2]),
        ("native-style-fat", [4096, 262144, 6206, 2, 1024, 5]),
        ("small-offset", [63, 131072, 64, 1, 1024, 2]),
        ("exact-offset-limit", [65503, 131072, 32, 1, 1024, 2]),
        ("alternate-root", [2048, 131072, 32, 1, 1024, 5]),
        ("chs-ceiling", [2048, 16500000, 32, 8, 16384, 2]),
        ("installed", [2048, 262144, 32, 1, 2048, 2]),
    ] {
        let directory = FixtureDirectory::new();
        let path = directory.0.join("disk.img");
        let mut args = vec![
            repo_root().display().to_string(),
            path.display().to_string(),
        ];
        args.extend(geometry.map(|value| value.to_string()));
        args.push(name.into());
        let output = python(CREATE, &args);
        let lines = output.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), 7);
        let extent = |index: usize| VolumeExtent {
            disk_index: 7,
            offset: lines[index].parse().unwrap(),
            length: lines[index + 1].parse().unwrap(),
        };
        let layout = DiskLayout {
            disk_index: 7,
            bytes: lines[0].parse().unwrap(),
            bytes_per_sector: 512,
            boot: extent(1),
            windows: extent(3),
        };
        let mut disk = FixtureFile::open(&path);
        let plan = build_bootstrap(&mut disk, &layout).unwrap();
        assert_eq!(plan.replacement, hex::<512>(lines[5]), "{name}");
        assert_eq!(plan.metadata, hex::<32>(lines[6]), "{name}");
        let mut backup = None;
        assert!(
            refresh_bootstrap(&mut disk, &layout, |sector| {
                backup = Some(*sector);
                Ok(())
            })
            .unwrap(),
            "{name}"
        );
        assert_eq!(backup, Some(plan.original));
        assert!(
            !refresh_bootstrap(&mut disk, &layout, |_| panic!(
                "idempotent oracle backed up"
            ))
            .unwrap()
        );
        assert_eq!(disk.writes, 1);
        python(
            VERIFY,
            &[
                repo_root().display().to_string(),
                path.display().to_string(),
                geometry[0].to_string(),
            ],
        );
    }
}
