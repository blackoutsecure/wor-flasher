package main

import (
	"bytes"
	"crypto/sha256"
	"encoding/binary"
	"errors"
	"fmt"
	"hash/crc32"
	"io"
	"math"
	"strconv"
	"strings"
)

const sectorSize = 512

var (
	efiType   = [16]byte{0x28, 0x73, 0x2a, 0xc1, 0x1f, 0xf8, 0xd2, 0x11, 0xba, 0x4b, 0, 0xa0, 0xc9, 0x3e, 0xc9, 0x3b}
	basicType = [16]byte{0xa2, 0xa0, 0xd0, 0xeb, 0xe5, 0xb9, 0x33, 0x44, 0x87, 0xc0, 0x68, 0xb6, 0xb7, 0x26, 0x99, 0xc7}
	le        = binary.LittleEndian
)

type installContext struct {
	diskIndex               uint32
	bootDrive, windowsDrive string
}

func parseContext(getenv func(string) string) (*installContext, error) {
	switch strings.ToUpper(getenv("WOR_DEVICE_TYPE")) {
	case "RPI4-ARM64", "RPI5-ARM64":
		return nil, nil
	case "RPI3-ARM64":
	default:
		return nil, errors.New("missing or unsupported WOR_DEVICE_TYPE; refusing to guess the target board")
	}
	switch strings.ToUpper(getenv("WOR_INSTALLOPTIONS_PARTITIONSCHEME")) {
	case "MBR":
		return nil, nil
	case "GPT":
	default:
		return nil, errors.New("missing or unsupported WoR partition scheme")
	}
	if !strings.EqualFold(getenv("WOR_IMAGE_ARCH"), "ARM64") {
		return nil, errors.New("the Pi 3 bootstrap requires a desktop ARM64 installation")
	}
	index := getenv("WOR_DISK_INDEX")
	if index == "" || strings.IndexFunc(index, func(c rune) bool { return c < '0' || c > '9' }) >= 0 {
		return nil, errors.New("WOR_DISK_INDEX must explicitly identify the selected disk")
	}
	number, err := strconv.ParseUint(index, 10, 32)
	if err != nil {
		return nil, fmt.Errorf("invalid WOR_DISK_INDEX: %w", err)
	}
	drive := func(name string) (string, error) {
		value := strings.ToUpper(getenv(name))
		if len(value) != 2 || value[0] < 'A' || value[0] > 'Z' || value[1] != ':' {
			return "", fmt.Errorf("%s must be a drive letter followed by a colon", name)
		}
		return value, nil
	}
	boot, err := drive("WOR_DISK_BOOTPARTITION")
	if err != nil {
		return nil, err
	}
	windows, err := drive("WOR_DISK_WINDOWSPARTITION")
	if err != nil {
		return nil, err
	}
	return &installContext{uint32(number), boot, windows}, nil
}

type volumeIdentity struct {
	drive                                 string
	deviceType, diskIndex, partitionIndex uint32
}

func selectBootDrive(context installContext, volumes []volumeIdentity) (string, error) {
	var selected string
	for _, volume := range volumes {
		if volume.deviceType != 7 || volume.diskIndex != context.diskIndex || volume.partitionIndex != 1 {
			continue
		}
		if len(volume.drive) != 2 || volume.drive[0] < 'A' || volume.drive[0] > 'Z' || volume.drive[1] != ':' {
			return "", errors.New("boot-volume discovery returned an invalid drive letter")
		}
		if volume.drive == context.windowsDrive {
			return "", errors.New("the Windows volume is the selected disk's boot partition")
		}
		if selected != "" {
			return "", errors.New("multiple mounted volumes claim the selected disk's first partition")
		}
		selected = volume.drive
	}
	if selected == "" {
		return "", fmt.Errorf("no mounted volume identifies partition 1 of selected disk %d", context.diskIndex)
	}
	if context.bootDrive != context.windowsDrive && selected != context.bootDrive {
		return "", errors.New("the exported boot drive does not identify the selected disk's first partition")
	}
	return selected, nil
}

type volumeExtent struct {
	diskIndex      uint32
	offset, length uint64
}

func validateDiskNumber(data []byte, expected uint32) error {
	if len(data) != 12 || le.Uint32(data) != 7 || le.Uint32(data[4:]) != expected {
		return errors.New("the raw device is not the selected physical disk")
	}
	partition := le.Uint32(data[8:])
	if partition != 0 && partition != math.MaxUint32 {
		return errors.New("the raw device is a partition, not a whole disk")
	}
	return nil
}

func parseVolumeExtent(data []byte) (volumeExtent, error) {
	// VOLUME_DISK_EXTENTS contains an eight-byte-aligned DISK_EXTENT on ARM64.
	if len(data) != 32 || le.Uint32(data) != 1 {
		return volumeExtent{}, errors.New("a target volume must have exactly one physical-disk extent")
	}
	extent := volumeExtent{le.Uint32(data[8:]), le.Uint64(data[16:]), le.Uint64(data[24:])}
	if extent.offset == 0 || extent.length == 0 || extent.offset > math.MaxInt64 ||
		extent.length > math.MaxInt64 || extent.offset%sectorSize != 0 || extent.length%sectorSize != 0 {
		return volumeExtent{}, errors.New("invalid or unaligned volume extent")
	}
	return extent, nil
}

type diskLayout struct {
	diskIndex      uint32
	bytes          uint64
	bytesPerSector uint32
	boot, windows  volumeExtent
}

func (layout diskLayout) validate() error {
	if layout.bytesPerSector != sectorSize || layout.bytes%sectorSize != 0 ||
		layout.bytes < 68*sectorSize || layout.bytes > math.MaxInt64 {
		return errors.New("Pi 3 GPT bootstrap requires a valid physical disk with 512-byte logical sectors")
	}
	for _, extent := range []volumeExtent{layout.boot, layout.windows} {
		if extent.diskIndex != layout.diskIndex || extent.offset == 0 || extent.length == 0 ||
			extent.offset%sectorSize != 0 || extent.length%sectorSize != 0 ||
			extent.offset >= layout.bytes || extent.length > layout.bytes-extent.offset {
			return errors.New("boot and Windows volume extents must belong to the selected physical disk")
		}
	}
	if layout.boot.offset+layout.boot.length > layout.windows.offset {
		return errors.New("Windows must follow, and not overlap, the boot partition")
	}
	return nil
}

func readSector(disk io.ReaderAt, lba uint64) ([sectorSize]byte, error) {
	var data [sectorSize]byte
	if lba > math.MaxInt64/sectorSize {
		return data, errors.New("sector offset exceeds the supported disk range")
	}
	n, err := disk.ReadAt(data[:], int64(lba*sectorSize))
	if err != nil {
		return data, fmt.Errorf("read sector %d: %w", lba, err)
	}
	if n != sectorSize {
		return data, fmt.Errorf("short read at sector %d: %d bytes", lba, n)
	}
	return data, nil
}

type bootstrapPlan struct {
	original, replacement [sectorSize]byte
	metadata              [sha256.Size]byte
}

func buildBootstrap(disk io.ReaderAt, layout diskLayout) (*bootstrapPlan, error) {
	if err := layout.validate(); err != nil {
		return nil, err
	}
	header, err := readSector(disk, 1)
	if err != nil {
		return nil, err
	}
	if string(header[:8]) != "EFI PART" {
		return nil, errors.New("target does not contain a GPT header at LBA 1")
	}
	headerSize := le.Uint32(header[12:])
	if headerSize < 92 || headerSize > sectorSize {
		return nil, errors.New("GPT header size is invalid")
	}
	checksumHeader := header
	le.PutUint32(checksumHeader[16:], 0)
	if crc32.ChecksumIEEE(checksumHeader[:headerSize]) != le.Uint32(header[16:]) {
		return nil, errors.New("GPT header checksum is invalid")
	}
	backupLBA := le.Uint64(header[32:])
	firstUsable, lastUsable := le.Uint64(header[40:]), le.Uint64(header[48:])
	if le.Uint64(header[24:]) != 1 || firstUsable < 34 || firstUsable > lastUsable ||
		lastUsable >= backupLBA || backupLBA != layout.bytes/sectorSize-1 {
		return nil, errors.New("GPT bounds do not match the selected physical disk")
	}
	entriesLBA := le.Uint64(header[72:])
	entryCount, entrySize := uint64(le.Uint32(header[80:])), uint64(le.Uint32(header[84:]))
	tableSize := entryCount * entrySize
	if entryCount == 0 || entrySize < 128 || entrySize%128 != 0 || entriesLBA < 2 ||
		entriesLBA >= firstUsable || (tableSize+sectorSize-1)/sectorSize > firstUsable-entriesLBA ||
		firstUsable > layout.boot.offset/sectorSize || layout.boot.offset/sectorSize > 65535 {
		return nil, errors.New("GPT partition-entry geometry is invalid for the Pi 3 bootstrap")
	}
	digest, checksum := sha256.New(), crc32.NewIEEE()
	digest.Write(header[:])
	bootMatches, windowsMatches := 0, 0
	for offset := uint64(0); offset < tableSize; offset += sectorSize {
		block, err := readSector(disk, entriesLBA+offset/sectorSize)
		if err != nil {
			return nil, err
		}
		count := min(uint64(sectorSize), tableSize-offset)
		checksum.Write(block[:count])
		digest.Write(block[:count])
		for within := uint64(0); within < count; within += 128 {
			if (offset+within)%entrySize != 0 {
				continue
			}
			entry := block[within : within+128]
			if bytes.Equal(entry[:16], make([]byte, 16)) {
				continue
			}
			start, end := le.Uint64(entry[32:]), le.Uint64(entry[40:])
			if start < firstUsable || start > end || end > lastUsable {
				return nil, errors.New("GPT partition has invalid bounds")
			}
			matches := func(extent volumeExtent) bool {
				return start*sectorSize == extent.offset && (end-start+1)*sectorSize == extent.length
			}
			if matches(layout.boot) {
				if offset+within != 0 || (!bytes.Equal(entry[:16], efiType[:]) && !bytes.Equal(entry[:16], basicType[:])) {
					return nil, errors.New("the boot volume must be GPT partition 1 with an EFI or Basic Data type")
				}
				bootMatches++
			}
			if matches(layout.windows) {
				if !bytes.Equal(entry[:16], basicType[:]) {
					return nil, errors.New("the Windows volume must be a Microsoft Basic Data partition")
				}
				windowsMatches++
			}
		}
	}
	if checksum.Sum32() != le.Uint32(header[88:]) {
		return nil, errors.New("GPT partition-entry checksum is invalid")
	}
	if bootMatches != 1 || windowsMatches != 1 {
		return nil, errors.New("GPT partitions do not uniquely match both selected volume extents")
	}

	startLBA := layout.boot.offset / sectorSize
	boot, err := readSector(disk, startLBA)
	if err != nil {
		return nil, err
	}
	digest.Write(boot[:])
	if boot[510] != 0x55 || boot[511] != 0xaa || le.Uint16(boot[11:]) != sectorSize {
		return nil, errors.New("FAT boot-sector signature or sector size is invalid")
	}
	clusterSize, reserved, fatCount := uint64(boot[13]), uint64(le.Uint16(boot[14:])), uint64(boot[16])
	total, fatSize := uint64(le.Uint32(boot[32:])), uint64(le.Uint32(boot[36:]))
	fatFlags, rootCluster := le.Uint16(boot[40:]), uint64(le.Uint32(boot[44:]))
	if clusterSize == 0 || clusterSize&(clusterSize-1) != 0 || reserved == 0 ||
		(fatCount != 1 && fatCount != 2) || fatSize == 0 {
		return nil, errors.New("FAT allocation geometry is invalid")
	}
	if le.Uint16(boot[17:]) != 0 || le.Uint16(boot[19:]) != 0 ||
		le.Uint16(boot[22:]) != 0 || le.Uint16(boot[42:]) != 0 {
		return nil, errors.New("boot partition must contain a FAT32 version 0 filesystem")
	}
	if fatFlags&0x80 != 0 && uint64(fatFlags&0xf) >= fatCount {
		return nil, errors.New("FAT32 active FAT index is invalid")
	}
	if total > layout.boot.length/sectorSize || total <= reserved+fatCount*fatSize {
		return nil, errors.New("FAT filesystem exceeds its partition bounds or has no data region")
	}
	clusters := (total - reserved - fatCount*fatSize) / clusterSize
	if clusters < 65525 || clusters >= 0x0ffffff5 || (clusters+2)*4 > fatSize*sectorSize ||
		rootCluster < 2 || rootCluster >= clusters+2 {
		return nil, errors.New("FAT32 data-cluster count, table size or root cluster is invalid")
	}
	if startLBA+reserved > 65535 || startLBA+total > math.MaxUint32 {
		return nil, errors.New("Pi 3 FAT bootstrap offsets exceed their 16-bit or 32-bit limits")
	}
	original, err := readSector(disk, 0)
	if err != nil {
		return nil, err
	}
	plan := &bootstrapPlan{original: original}
	copy(plan.metadata[:], digest.Sum(nil))
	copy(plan.replacement[440:446], original[440:446])
	copy(plan.replacement[11:90], boot[11:90])
	le.PutUint16(plan.replacement[14:], uint16(startLBA+reserved))
	le.PutUint32(plan.replacement[32:], uint32(startLBA+total))
	for _, offset := range []int{48, 50} {
		relative := uint64(le.Uint16(boot[offset:]))
		if relative != 0 && relative != 0xffff {
			if relative >= reserved {
				return nil, errors.New("FAT32 metadata pointer is outside the reserved region")
			}
			le.PutUint16(plan.replacement[offset:], uint16(startLBA+relative))
		}
	}
	// Match the desktop helper's protective-only MBR, never a hybrid MBR.
	protectiveSize := min(backupLBA, uint64(math.MaxUint32))
	firstCHS, lastCHS := lbaToCHS(1), lbaToCHS(protectiveSize)
	copy(plan.replacement[447:450], firstCHS[:])
	plan.replacement[450] = 0xee
	copy(plan.replacement[451:454], lastCHS[:])
	le.PutUint32(plan.replacement[454:], 1)
	le.PutUint32(plan.replacement[458:], uint32(protectiveSize))
	plan.replacement[510], plan.replacement[511] = 0x55, 0xaa
	return plan, nil
}

func lbaToCHS(lba uint64) [3]byte {
	sector, track := lba%63+1, lba/63
	head, cylinder := track%255, track/255
	if cylinder > 1023 {
		return [3]byte{0xfe, 0xff, 0xff}
	}
	return [3]byte{byte(head), byte(sector | ((cylinder >> 2) & 0xc0)), byte(cylinder)}
}

type bootstrapDisk interface {
	io.ReaderAt
	io.WriterAt
	Sync() error
}

func refreshBootstrap(disk bootstrapDisk, layout diskLayout, backup func([]byte) error) (bool, error) {
	plan, err := buildBootstrap(disk, layout)
	if err != nil {
		return false, err
	}
	if plan.original == plan.replacement {
		return false, nil
	}
	if err := backup(plan.original[:]); err != nil {
		return false, fmt.Errorf("boot-sector backup failed; no boot-sector write attempted: %w", err)
	}
	current, err := buildBootstrap(disk, layout)
	if err != nil {
		return false, err
	}
	if *current != *plan {
		return false, errors.New("disk metadata changed while backing up; no boot-sector write attempted")
	}
	n, err := disk.WriteAt(plan.replacement[:], 0)
	if err != nil {
		return false, fmt.Errorf("boot-sector write failed; preserve the backup and do not reboot: %w", err)
	}
	if n != sectorSize {
		return false, fmt.Errorf("short boot-sector write: %d of 512 bytes; do not reboot", n)
	}
	if err := disk.Sync(); err != nil {
		return false, fmt.Errorf("boot-sector flush failed; do not reboot: %w", err)
	}
	verified, err := buildBootstrap(disk, layout)
	if err != nil {
		return false, fmt.Errorf("post-write validation failed; do not reboot: %w", err)
	}
	if verified.original != plan.replacement || verified.replacement != plan.replacement ||
		verified.metadata != plan.metadata {
		return false, errors.New("boot-sector read-back or GPT/FAT metadata verification failed; do not reboot")
	}
	return true, nil
}
