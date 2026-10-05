package main

import (
	"bytes"
	"crypto/sha256"
	"encoding/json"
	"errors"
	"io"
	"math"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

const fixturePython = `
import json, runpy, struct, sys
from pathlib import Path
ns = runpy.run_path("../../tests/test-pi3-gpt-bootstrap.py")
path = Path(sys.argv[1])
options = json.loads(sys.argv[2])
temporary_partition = options.pop("temporary_partition", 0)
if options.pop("boot_basic_data", 0):
    options["partition_type"] = ns["BASIC_DATA_TYPE"]
before = None
old_size = path.stat().st_size if path.exists() else 0
if old_size:
    with path.open("rb") as source:
        before = source.read(512)
ns["create_gpt_image"](path, **options)
if old_size:
    with path.open("r+b") as disk:
        disk.truncate(old_size)
        disk.write(before)
        disk.seek(512)
        header = bytearray(disk.read(512))
        disk.seek(1024)
        entries = bytearray(disk.read(16384))
        backup_lba = old_size // 512 - 1
        boot_end = struct.unpack_from("<Q", entries, 40)[0]
        windows_start = boot_end + 1 + 32768
        entries[256:384] = entries[128:256]
        entries[128:256] = bytes(128)
        entries[128:144] = bytes.fromhex("16e3c9e35c0bb84d817df92df00215ae")
        entries[144:160] = b"\x44" * 16
        struct.pack_into("<QQ", entries, 160, boot_end + 1, windows_start - 1)
        struct.pack_into("<QQ", entries, 288, windows_start, backup_lba - 33)
        if temporary_partition:
            temporary_start = backup_lba - 32768
            entries[384:512] = entries[256:384]
            entries[256:384] = bytes(128)
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
`

func runPython(t *testing.T, code string, args ...string) []byte {
	t.Helper()
	command := exec.Command("python3", append([]string{"-B", "-c", code}, args...)...)
	result, err := command.CombinedOutput()
	if err != nil {
		t.Fatalf("Python fixture/helper failed: %v\n%s", err, result)
	}
	return result
}

func makeFixture(t *testing.T, path string, options map[string]int) (*os.File, diskLayout) {
	t.Helper()
	data, err := json.Marshal(options)
	if err != nil {
		t.Fatal(err)
	}
	if options == nil {
		data = []byte("{}")
	}
	runPython(t, fixturePython, path, string(data))
	disk, err := os.OpenFile(path, os.O_RDWR, 0600)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { disk.Close() })
	info, err := disk.Stat()
	if err != nil {
		t.Fatal(err)
	}
	table, err := readSector(disk, 2)
	if err != nil {
		t.Fatal(err)
	}
	extent := func(entry []byte) volumeExtent {
		start, end := le.Uint64(entry[32:]), le.Uint64(entry[40:])
		return volumeExtent{7, start * sectorSize, (end - start + 1) * sectorSize}
	}
	layout := diskLayout{diskIndex: 7, bytes: uint64(info.Size()), bytesPerSector: 512, boot: extent(table[:128])}
	for offset := 128; offset < sectorSize; offset += 128 {
		if bytes.Equal(table[offset:offset+16], basicType[:]) {
			layout.windows = extent(table[offset : offset+128])
		}
	}
	return disk, layout
}

func fixture(t *testing.T, options map[string]int) (*os.File, diskLayout) {
	t.Helper()
	return makeFixture(t, filepath.Join(t.TempDir(), "disk.img"), options)
}

type checkedDisk struct {
	*os.File
	t          *testing.T
	writes     int
	writeSize  int
	dropWrite  bool
	shortWrite bool
	writeError bool
	syncError  bool
	afterWrite func()
}

func (disk *checkedDisk) ReadAt(data []byte, offset int64) (int, error) {
	if len(data) != sectorSize || offset%sectorSize != 0 {
		disk.t.Fatalf("unaligned raw-device read: %d bytes at %d", len(data), offset)
	}
	return disk.File.ReadAt(data, offset)
}

func (disk *checkedDisk) WriteAt(data []byte, offset int64) (int, error) {
	disk.writes++
	disk.writeSize = len(data)
	if offset != 0 || len(data) != sectorSize {
		disk.t.Fatalf("write outside sector 0: %d bytes at %d", len(data), offset)
	}
	if disk.writeError {
		return 0, errors.New("fixture write error")
	}
	if disk.shortWrite {
		return 511, nil
	}
	if disk.dropWrite {
		return len(data), nil
	}
	n, err := disk.File.WriteAt(data, offset)
	if disk.afterWrite != nil {
		disk.afterWrite()
	}
	return n, err
}

func (disk *checkedDisk) Sync() error {
	if disk.syncError {
		return errors.New("fixture flush error")
	}
	return disk.File.Sync()
}

func TestBootstrapMatchesDesktopHelper(t *testing.T) {
	for name, options := range map[string]map[string]int{
		"default":            nil,
		"installed-128-MiB":  {"sector_count": 262144, "fat_sectors": 2048},
		"installer-1536-MiB": {"sector_count": 3145728, "sectors_per_cluster": 8, "fat_sectors": 4096},
		"alternate-offset":   {"start_lba": 4096, "reserved_sectors": 64},
		"exact-offset-limit": {"start_lba": 65503},
		"alternate-root":     {"root_cluster": 5},
	} {
		t.Run(name, func(t *testing.T) {
			file, layout := fixture(t, options)
			disk := &checkedDisk{File: file, t: t}
			plan, err := buildBootstrap(disk, layout)
			if err != nil {
				t.Fatal(err)
			}
			runPython(t, `
import runpy, sys
sys.argv = ["pi3-hybrid-mbr.py", sys.argv[1]]
runpy.run_path("../../src/lib/pi3-hybrid-mbr.py", run_name="__main__")
`, file.Name())
			expected, err := readSector(disk, 0)
			if err != nil {
				t.Fatal(err)
			}
			if plan.replacement != expected {
				t.Fatal("native bootstrap differs from the hardware-confirmed desktop helper")
			}
		})
	}
}

func TestRefreshAfterWindowsRecreatesBootPartition(t *testing.T) {
	file, originalLayout := fixture(t, map[string]int{
		"sector_count": 3145728, "sectors_per_cluster": 8, "fat_sectors": 4096,
	})
	if _, err := refreshBootstrap(file, originalLayout, func([]byte) error { return nil }); err != nil {
		t.Fatal(err)
	}
	_, installedLayout := makeFixture(t, file.Name(), map[string]int{
		"sector_count": 262144, "fat_sectors": 2048, "boot_basic_data": 1, "temporary_partition": 1,
	})
	stale, err := readSector(file, 0)
	if err != nil {
		t.Fatal(err)
	}
	if le.Uint32(stale[32:]) != 3145728+2048 || originalLayout.bytes != installedLayout.bytes {
		t.Fatal("fixture did not retain the old bootstrap while resizing partitions on the same disk")
	}
	disk := &checkedDisk{File: file, t: t}
	var backup []byte
	changed, err := refreshBootstrap(disk, installedLayout, func(data []byte) error {
		backup = bytes.Clone(data)
		return nil
	})
	if err != nil || !changed || disk.writes != 1 || !bytes.Equal(backup, stale[:]) {
		t.Fatalf("post-install repair failed: changed=%v writes=%d err=%v", changed, disk.writes, err)
	}
	repaired, err := readSector(file, 0)
	if err != nil {
		t.Fatal(err)
	}
	if le.Uint32(repaired[32:]) != 262144+2048 || le.Uint16(repaired[14:]) != 2048+32 {
		t.Fatal("bootstrap still describes the installer filesystem instead of the installed filesystem")
	}
	runPython(t, `
import runpy, sys
ns = runpy.run_path("../../tests/test-pi3-gpt-bootstrap.py")
path = ns["Path"](sys.argv[1])
assert ns["read_firmware_file"](path, 0) == ns["read_firmware_file"](path, 2048)
sys.argv = ["pi3-hybrid-mbr.py", "--check", str(path)]
runpy.run_path("../../src/lib/pi3-hybrid-mbr.py", run_name="__main__")
`, file.Name())
}

func TestOnlySectorZeroChangesAndSecondRunIsReadOnly(t *testing.T) {
	file, layout := fixture(t, nil)
	hashRemainder := func() [sha256.Size]byte {
		hash := sha256.New()
		if _, err := io.Copy(hash, io.NewSectionReader(file, 512, int64(layout.bytes)-512)); err != nil {
			t.Fatal(err)
		}
		var result [sha256.Size]byte
		copy(result[:], hash.Sum(nil))
		return result
	}
	before := hashRemainder()
	disk := &checkedDisk{File: file, t: t}
	backups := 0
	backup := func(data []byte) error {
		backups++
		if len(data) != 512 || disk.writes != 0 {
			t.Fatal("backup was not completed before the only device write")
		}
		return nil
	}
	changed, err := refreshBootstrap(disk, layout, backup)
	if err != nil || !changed || disk.writes != 1 || disk.writeSize != 512 || backups != 1 {
		t.Fatalf("unexpected repair result: changed=%v writes=%d backups=%d err=%v", changed, disk.writes, backups, err)
	}
	if hashRemainder() != before {
		t.Fatal("bytes outside sector 0 changed")
	}
	changed, err = refreshBootstrap(disk, layout, backup)
	if err != nil || changed || disk.writes != 1 || backups != 1 {
		t.Fatal("repeat finalization was not read-only and idempotent")
	}
}

func TestInvalidTargetsAreRejectedBeforeBackupOrWrite(t *testing.T) {
	for _, name := range []string{
		"wrong-boot-disk", "wrong-windows-disk", "wrong-boot-offset", "wrong-windows-length",
		"overlap", "4096-byte-sector", "wrong-disk-length", "gpt-checksum", "table-checksum",
		"fat-signature", "fat16", "fat-active-index", "fat-data-underflow", "fat-root",
		"fat-pointer", "bootstrap-offset-overflow",
	} {
		t.Run(name, func(t *testing.T) {
			options := map[string]int{}
			if name == "bootstrap-offset-overflow" {
				options["start_lba"] = 65520
			}
			file, layout := fixture(t, options)
			write := func(offset int64, data []byte) {
				if _, err := file.WriteAt(data, offset); err != nil {
					t.Fatal(err)
				}
			}
			switch name {
			case "wrong-boot-disk":
				layout.boot.diskIndex++
			case "wrong-windows-disk":
				layout.windows.diskIndex++
			case "wrong-boot-offset":
				layout.boot.offset += 512
			case "wrong-windows-length":
				layout.windows.length -= 512
			case "overlap":
				layout.windows.offset = layout.boot.offset
			case "4096-byte-sector":
				layout.bytesPerSector = 4096
			case "wrong-disk-length":
				layout.bytes += 512
			case "gpt-checksum":
				write(512+56, []byte{0xff})
			case "table-checksum":
				write(1024+16, []byte{0xff})
			case "fat-signature":
				write(int64(layout.boot.offset)+510, []byte{0, 0})
			case "fat16":
				write(int64(layout.boot.offset)+22, []byte{1, 0})
			case "fat-active-index":
				write(int64(layout.boot.offset)+40, []byte{0x82, 0})
			case "fat-data-underflow":
				write(int64(layout.boot.offset)+32, []byte{1, 0, 0, 0})
			case "fat-root":
				write(int64(layout.boot.offset)+44, []byte{1, 0, 0, 0})
			case "fat-pointer":
				write(int64(layout.boot.offset)+48, []byte{32, 0})
			}
			disk := &checkedDisk{File: file, t: t}
			called := false
			changed, err := refreshBootstrap(disk, layout, func([]byte) error { called = true; return nil })
			if err == nil || changed || called || disk.writes != 0 {
				t.Fatalf("unsafe target reached backup/write: changed=%v backup=%v writes=%d err=%v", changed, called, disk.writes, err)
			}
		})
	}
}

func TestBackupAndWriteFailuresCannotReportSuccess(t *testing.T) {
	for _, name := range []string{"backup", "metadata-changed", "short-write", "write-error", "flush-error", "readback", "gpt-changed"} {
		t.Run(name, func(t *testing.T) {
			file, layout := fixture(t, nil)
			disk := &checkedDisk{File: file, t: t}
			backup := func([]byte) error { return nil }
			switch name {
			case "backup":
				backup = func([]byte) error { return errors.New("fixture backup error") }
			case "metadata-changed":
				backup = func([]byte) error { _, err := file.WriteAt([]byte{0xff}, 440); return err }
			case "short-write":
				disk.shortWrite = true
			case "write-error":
				disk.writeError = true
			case "flush-error":
				disk.syncError = true
			case "readback":
				disk.dropWrite = true
			case "gpt-changed":
				disk.afterWrite = func() {
					if _, err := file.WriteAt([]byte{0xff}, 512+56); err != nil {
						t.Fatal(err)
					}
				}
			}
			changed, err := refreshBootstrap(disk, layout, backup)
			if err == nil || changed {
				t.Fatalf("failure reported success: changed=%v err=%v", changed, err)
			}
			if (name == "backup" || name == "metadata-changed") && disk.writes != 0 {
				t.Fatal("device write occurred before a valid stable backup")
			}
		})
	}
}

func TestWoR11DuplicateDriveExport(t *testing.T) {
	values := map[string]string{
		"WOR_DEVICE_TYPE": "RPi3-ARM64", "WOR_INSTALLOPTIONS_PARTITIONSCHEME": "GPT",
		"WOR_IMAGE_ARCH": "ARM64", "WOR_DISK_INDEX": "0",
		"WOR_DISK_BOOTPARTITION": "D:", "WOR_DISK_WINDOWSPARTITION": "D:",
	}
	context, err := parseContext(func(name string) string { return values[name] })
	if err != nil {
		t.Fatalf("WoR-PE 1.1 duplicates the Windows letter; boot must be resolved from disk metadata: %v", err)
	}
	if context.diskIndex != 0 || context.bootDrive != "D:" || context.windowsDrive != "D:" {
		t.Fatalf("the original context must be retained until physical-volume validation: %+v", context)
	}
}

func TestWoRContextNeverDefaultsToDiskZero(t *testing.T) {
	valid := map[string]string{
		"WOR_DEVICE_TYPE": "RPi3-ARM64", "WOR_INSTALLOPTIONS_PARTITIONSCHEME": "GPT",
		"WOR_IMAGE_ARCH": "ARM64", "WOR_DISK_INDEX": "7",
		"WOR_DISK_BOOTPARTITION": "B:", "WOR_DISK_WINDOWSPARTITION": "W:",
	}
	for name, values := range map[string][]string{
		"WOR_DEVICE_TYPE":                    {"", "RPi3-ARM32", "unknown"},
		"WOR_INSTALLOPTIONS_PARTITIONSCHEME": {"", "unknown"},
		"WOR_IMAGE_ARCH":                     {"", "x64", "ARM"},
		"WOR_DISK_INDEX":                     {"", "-1", "+1", "1&exit", " 1", "4294967296"},
		"WOR_DISK_BOOTPARTITION":             {"", `B:\`, "B:folder", `\\.\PhysicalDrive0`},
		"WOR_DISK_WINDOWSPARTITION":          {"", `W:\`},
	} {
		for _, value := range values {
			t.Run(name+"="+value, func(t *testing.T) {
				_, err := parseContext(func(key string) string {
					if key == name {
						return value
					}
					return valid[key]
				})
				if err == nil {
					t.Fatal("invalid installation context was accepted")
				}
			})
		}
	}
	context, err := parseContext(func(name string) string { return valid[name] })
	if err != nil || context.diskIndex != 7 || context.bootDrive != "B:" || context.windowsDrive != "W:" {
		t.Fatalf("valid recovery/self-install destination rejected: %+v %v", context, err)
	}
	for key, values := range map[string][]string{
		"WOR_DEVICE_TYPE":                    {"RPi4-ARM64", "RPi5-ARM64"},
		"WOR_INSTALLOPTIONS_PARTITIONSCHEME": {"MBR"},
	} {
		for _, value := range values {
			context, err := parseContext(func(name string) string {
				if name == key {
					return value
				}
				return valid[name]
			})
			if err != nil || context != nil {
				t.Fatal("unrelated board/MBR should require no disk access")
			}
		}
	}
}

func TestBootDriveDiscovery(t *testing.T) {
	context := installContext{diskIndex: 0, bootDrive: "D:", windowsDrive: "D:"}
	volumes := []volumeIdentity{
		{"A:", 7, 9, 1},
		{"B:", 7, 0, 1},
		{"C:", 7, 0, 3},
		{"D:", 7, 0, 4},
		{"X:", 0x24, 0, 1},
	}
	boot, err := selectBootDrive(context, volumes)
	if err != nil || boot != "B:" {
		t.Fatalf("actual WoR-PE 1.1 layout was not resolved safely: boot=%s err=%v", boot, err)
	}
	for _, name := range []string{"wrong-disk", "missing-boot", "ambiguous", "windows-is-boot", "invalid-drive", "wrong-export"} {
		t.Run(name, func(t *testing.T) {
			current := context
			candidates := append([]volumeIdentity(nil), volumes...)
			switch name {
			case "wrong-disk":
				candidates[1].diskIndex = 2
			case "missing-boot":
				candidates[1].partitionIndex = 2
			case "ambiguous":
				candidates = append(candidates, volumeIdentity{"E:", 7, 0, 1})
			case "windows-is-boot":
				candidates[3].partitionIndex = 1
			case "invalid-drive":
				candidates[1].drive = `B:\other`
			case "wrong-export":
				current.bootDrive = "C:"
			}
			if _, err := selectBootDrive(current, candidates); err == nil {
				t.Fatal("unsafe or ambiguous boot context was accepted")
			}
		})
	}
	context.bootDrive = "B:"
	if boot, err := selectBootDrive(context, volumes[1:2]); err != nil || boot != "B:" {
		t.Fatal("a correct export should still use the validated explicit boot drive")
	}
	context = installContext{diskIndex: 7, bootDrive: "Z:", windowsDrive: "Z:"}
	boot, err = selectBootDrive(context, []volumeIdentity{{"B:", 7, 0, 1}, {"R:", 7, 7, 1}, {"Z:", 7, 7, 3}})
	if err != nil || boot != "R:" {
		t.Fatal("recovery-media discovery must follow the selected target, not disk 0 or a fixed B: letter")
	}
}

func TestDistinctDriveLettersCannotAliasOnePartition(t *testing.T) {
	file, layout := fixture(t, nil)
	layout.windows = layout.boot
	disk := &checkedDisk{File: file, t: t}
	backups := 0
	changed, err := refreshBootstrap(disk, layout, func([]byte) error { backups++; return nil })
	if err == nil || changed || backups != 0 || disk.writes != 0 {
		t.Fatal("physically identical boot/Windows volumes reached backup or write")
	}
}

func TestWindowsDeviceControlLayouts(t *testing.T) {
	data := make([]byte, 32)
	le.PutUint32(data, 1)
	le.PutUint32(data[8:], 7)
	le.PutUint64(data[16:], 2048*512)
	le.PutUint64(data[24:], 128*1024*1024)
	extent, err := parseVolumeExtent(data)
	if err != nil || extent != (volumeExtent{7, 1048576, 134217728}) {
		t.Fatalf("ARM64 volume extent parsed incorrectly: %+v %v", extent, err)
	}
	for _, count := range []uint32{0, 2} {
		le.PutUint32(data, count)
		if _, err := parseVolumeExtent(data); err == nil {
			t.Fatal("multi-disk/empty volume accepted")
		}
	}
	if _, err := parseVolumeExtent(data[:31]); err == nil {
		t.Fatal("truncated volume extent accepted")
	}
	device := make([]byte, 12)
	le.PutUint32(device, 7)
	le.PutUint32(device[4:], 7)
	for _, partition := range []uint32{0, math.MaxUint32} {
		le.PutUint32(device[8:], partition)
		if err := validateDiskNumber(device, 7); err != nil {
			t.Fatal(err)
		}
	}
	le.PutUint32(device[8:], 1)
	if err := validateDiskNumber(device, 7); err == nil || !strings.Contains(err.Error(), "whole disk") {
		t.Fatal("partition handle accepted as a whole disk")
	}
	le.PutUint32(device[8:], 0)
	if err := validateDiskNumber(device, 8); err == nil {
		t.Fatal("wrong physical disk accepted")
	}
}
