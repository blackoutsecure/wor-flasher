package main

import (
	"errors"
	"fmt"
	"os"
	"strings"
	"syscall"
)

const (
	ioctlDiskGeometry  = 0x70000
	ioctlDiskLength    = 0x7405c
	ioctlStorageNumber = 0x2d1080
	ioctlVolumeExtents = 0x560000
)

func deviceInfo(file *os.File, control uint32, size int) ([]byte, error) {
	data := make([]byte, size)
	var returned uint32
	err := syscall.DeviceIoControl(syscall.Handle(file.Fd()), control, nil, 0, &data[0], uint32(size), &returned, nil)
	if err != nil {
		return nil, fmt.Errorf("%s ioctl %#x: %w", file.Name(), control, err)
	}
	if returned != uint32(size) {
		return nil, fmt.Errorf("%s ioctl %#x returned %d bytes, expected %d", file.Name(), control, returned, size)
	}
	return data, nil
}

func requireFile(path string) error {
	info, err := os.Stat(path)
	if err != nil {
		return err
	}
	if !info.Mode().IsRegular() || info.Size() == 0 {
		return fmt.Errorf("required installation file is absent or empty: %s", path)
	}
	return nil
}

func identifyVolume(file *os.File, drive string) (volumeIdentity, error) {
	data, err := deviceInfo(file, ioctlStorageNumber, 12)
	if err != nil {
		return volumeIdentity{}, err
	}
	return volumeIdentity{drive, le.Uint32(data), le.Uint32(data[4:]), le.Uint32(data[8:])}, nil
}

func probeVolume(drive string) (identity volumeIdentity, err error) {
	file, err := os.Open(`\\.\` + drive)
	if err != nil {
		return identity, err
	}
	defer func() { err = errors.Join(err, file.Close()) }()
	return identifyVolume(file, drive)
}

func discoverBootDrive(context installContext) (string, error) {
	drives := []string{context.bootDrive}
	duplicateExport := context.bootDrive == context.windowsDrive
	if duplicateExport {
		// WoR-PE 1.1 exports WindowsPartitionLetter for BOTH partition variables.
		// Discover the selected disk's first partition, never an adjacent letter.
		event("LogWarn", fmt.Sprintf("WoR-PE exported %s for both boot and Windows; locating partition 1 of disk %d.", context.windowsDrive, context.diskIndex))
		mask, _, err := syscall.NewLazyDLL("kernel32.dll").NewProc("GetLogicalDrives").Call()
		if mask == 0 {
			if err != syscall.Errno(0) {
				return "", fmt.Errorf("enumerating mounted drive letters: %w", err)
			}
			return "", errors.New("GetLogicalDrives returned no mounted volumes")
		}
		drives = nil
		for index := 0; index < 26; index++ {
			if mask&(1<<index) != 0 {
				drives = append(drives, string(rune('A'+index))+":")
			}
		}
	}
	var volumes []volumeIdentity
	for _, drive := range drives {
		identity, err := probeVolume(drive)
		if err != nil {
			if !duplicateExport {
				return "", err
			}
			event("LogDebug", fmt.Sprintf("Boot-volume discovery could not identify %s: %v", drive, err))
			continue
		}
		volumes = append(volumes, identity)
	}
	return selectBootDrive(context, volumes)
}

func saveBackup(directory string, sector []byte) (err error) {
	if err := os.MkdirAll(directory, 0700); err != nil {
		return err
	}
	file, err := os.CreateTemp(directory, "pi3-sector0-before-*.bin")
	if err != nil {
		return err
	}
	defer func() { err = errors.Join(err, file.Close()) }()
	if n, err := file.Write(sector); err != nil {
		return err
	} else if n != sectorSize {
		return fmt.Errorf("short backup write to %s: %d bytes", file.Name(), n)
	}
	if err := file.Sync(); err != nil {
		return err
	}
	event("LogInfo", "Original Pi 3 boot sector backed up to "+file.Name())
	return nil
}

func run() (message string, err error) {
	context, err := parseContext(os.Getenv)
	if err != nil {
		return "", err
	}
	if context == nil {
		return "Pi 3 GPT bootstrap is not applicable to this board or MBR installation; no disk accessed.", nil
	}
	miniNT, err := syscall.UTF16PtrFromString(`SYSTEM\CurrentControlSet\Control\MiniNT`)
	if err != nil {
		return "", err
	}
	var key syscall.Handle
	if err := syscall.RegOpenKeyEx(syscall.HKEY_LOCAL_MACHINE, miniNT, 0, syscall.KEY_QUERY_VALUE, &key); err != nil {
		return "", fmt.Errorf("automatic boot refresh must run in Windows PE: %w", err)
	}
	if err := syscall.RegCloseKey(key); err != nil {
		return "", err
	}
	windows, err := os.Open(`\\.\` + context.windowsDrive)
	if err != nil {
		return "", err
	}
	defer func() { err = errors.Join(err, windows.Close()) }()
	windowsData, err := deviceInfo(windows, ioctlVolumeExtents, 32)
	if err != nil {
		return "", err
	}
	windowsExtent, err := parseVolumeExtent(windowsData)
	if err != nil {
		return "", err
	}
	if windowsExtent.diskIndex != context.diskIndex {
		return "", errors.New("the Windows volume does not belong to the selected disk")
	}
	if err := requireFile(context.windowsDrive + `\Windows\System32\ntoskrnl.exe`); err != nil {
		return "", fmt.Errorf("the selected Windows installation is not ready for finalization: %w", err)
	}
	bootDrive, err := discoverBootDrive(*context)
	if err != nil {
		return "", err
	}
	context.bootDrive = bootDrive
	event("LogInfo", fmt.Sprintf("Verified destination volumes: boot %s, Windows %s, disk %d.", context.bootDrive, context.windowsDrive, context.diskIndex))
	if err := requireFile(context.bootDrive + `\RPI_EFI.fd`); err != nil {
		return "", fmt.Errorf("the installed boot partition is missing Pi 3 firmware: %w", err)
	}
	boot, err := os.OpenFile(`\\.\`+context.bootDrive, os.O_RDWR, 0)
	if err != nil {
		return "", err
	}
	defer func() { err = errors.Join(err, boot.Close()) }()
	identity, err := identifyVolume(boot, context.bootDrive)
	if err != nil {
		return "", err
	}
	if _, err := selectBootDrive(*context, []volumeIdentity{identity}); err != nil {
		return "", fmt.Errorf("boot-volume identity changed after discovery: %w", err)
	}
	layout := diskLayout{diskIndex: context.diskIndex}
	for _, volume := range []struct {
		file   *os.File
		extent *volumeExtent
	}{{boot, &layout.boot}, {windows, &layout.windows}} {
		data, err := deviceInfo(volume.file, ioctlVolumeExtents, 32)
		if err != nil {
			return "", err
		}
		*volume.extent, err = parseVolumeExtent(data)
		if err != nil {
			return "", err
		}
		if volume.extent.diskIndex != context.diskIndex {
			return "", errors.New("WoR boot/Windows drive letters do not belong to the selected disk")
		}
	}
	disk, err := os.OpenFile(fmt.Sprintf(`\\.\PhysicalDrive%d`, context.diskIndex), os.O_RDWR, 0)
	if err != nil {
		return "", err
	}
	defer func() { err = errors.Join(err, disk.Close()) }()
	number, err := deviceInfo(disk, ioctlStorageNumber, 12)
	if err != nil {
		return "", err
	}
	if err := validateDiskNumber(number, context.diskIndex); err != nil {
		return "", err
	}
	geometry, err := deviceInfo(disk, ioctlDiskGeometry, 24)
	if err != nil {
		return "", err
	}
	length, err := deviceInfo(disk, ioctlDiskLength, 8)
	if err != nil {
		return "", err
	}
	layout.bytesPerSector, layout.bytes = le.Uint32(geometry[20:]), le.Uint64(length)
	if err := layout.validate(); err != nil {
		return "", err
	}
	if err := boot.Sync(); err != nil {
		return "", fmt.Errorf("cannot flush the installed boot filesystem before inspecting its geometry: %w", err)
	}
	event("LogInfo", "Refreshing the Pi 3 bootstrap for the installed boot partition before reboot.")
	// Sector 0 is outside all validated partition extents; Windows permits this
	// disk-handle write without dismounting the volumes the WoR hook still needs.
	changed, err := refreshBootstrap(disk, layout, func(sector []byte) error {
		return saveBackup(context.windowsDrive+`\Windows\Logs\WoR-Flasher`, sector)
	})
	if err != nil {
		return "", err
	}
	if !changed {
		return "Pi 3 GPT-compatible bootstrap already matches the installed filesystem; no write needed.", nil
	}
	return "Pi 3 installed boot-sector repair verified; GPT and FAT metadata are unchanged.", nil
}

func event(level, message string) {
	message = strings.NewReplacer("|", "/", "\r", " ", "\n", " ").Replace(message)
	fmt.Printf("WoR-Event|%s|%s\n", level, message)
}

func main() {
	message, err := run()
	if err != nil {
		event("LogFatal", "Pi 3 boot finalization failed: "+err.Error())
		os.Exit(1)
	}
	event("LogInfo", message)
}
