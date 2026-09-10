::
:: Pre-finalization script for the Windows on Raspberry PE-based installer.
::
:: WoR-PE applies install.wim with DISM rather than running Windows Setup's media flow, so the
:: Autounattend.xml written to the root of the media is never read - nothing performs the implicit
:: answer-file search. This hook runs just before the installer finalizes, with the freshly applied
:: Windows partition still mounted, and drops the answer file into the one location the installed
:: OS does read on first boot: %WINDIR%\Panther\unattend.xml.
::
:: Documented at https://worproject.com/guides/wor-imager-customization
::
@echo off

set answerSource=%~dp0unattend.xml
set pantherDir=%WOR_DISK_WINDOWSPARTITION%\Windows\Panther

if not exist "%answerSource%" (
  call :RaiseEvent LogWarn, "No answer file to install; leaving Windows setup unattended settings alone."
  goto :end
)

if not exist "%WOR_DISK_WINDOWSPARTITION%\Windows\" (
  call :RaiseEvent LogWarn, "No Windows directory on %WOR_DISK_WINDOWSPARTITION%; skipping answer file."
  goto :end
)

call :RaiseEvent LogInfo, "Installing offline setup answer file..."

if not exist "%pantherDir%\" mkdir "%pantherDir%"

copy /y "%answerSource%" "%pantherDir%\unattend.xml" >nul
if errorlevel 1 (
  call :RaiseEvent LogWarn, "Could not write the answer file; Windows setup will ask for a network."
  goto :end
)

call :RaiseEvent LogInfo, "Answer file installed to %pantherDir%\unattend.xml"

::
:: The Pi 4 RAM-unlock action runs during specialize, on the installed OS. Copying it there too
:: means it does not depend on the installation media still being visible and lettered by then.
::
set ramUnlockSource=%~dp0Pi4Disable3GB.ps1
set scriptsDir=%WOR_DISK_WINDOWSPARTITION%\Windows\Setup\Scripts

if not exist "%ramUnlockSource%" goto :stage_shell

if not exist "%scriptsDir%\" mkdir "%scriptsDir%"

copy /y "%ramUnlockSource%" "%scriptsDir%\Pi4Disable3GB.ps1" >nul
if errorlevel 1 (
  call :RaiseEvent LogWarn, "Could not install the Pi 4 RAM unlock; the 3 GB limit will stay enabled."
  goto :end
)

call :RaiseEvent LogInfo, "Pi 4 RAM unlock installed to %scriptsDir%\Pi4Disable3GB.ps1"

:: Optional one-time UEFI Shell handoff. The shell restores BOOTAA64.EFI before chain-loading it.
:stage_shell
set shellSource=%~dp0Shell.efi
set bootDir=%WOR_DISK_BOOTPARTITION%
if not exist "%shellSource%" goto :end
if not exist "%bootDir%\EFI\BOOT\BOOTAA64.EFI" (
  call :RaiseEvent LogWarn, "Could not find EFI\BOOT\BOOTAA64.EFI; skipping the automatic UEFI Shell handoff."
  goto :end
)
if exist "%bootDir%\EFI\BOOT\BOOTAA64.WOR" (
  call :RaiseEvent LogWarn, "A previous UEFI Shell handoff backup exists; refusing to replace it."
  goto :end
)
copy /y "%bootDir%\EFI\BOOT\BOOTAA64.EFI" "%bootDir%\EFI\BOOT\BOOTAA64.WOR" >nul
if errorlevel 1 goto :end
copy /y "%shellSource%" "%bootDir%\EFI\BOOT\BOOTAA64.EFI" >nul
if errorlevel 1 (
  copy /y "%bootDir%\EFI\BOOT\BOOTAA64.WOR" "%bootDir%\EFI\BOOT\BOOTAA64.EFI" >nul
  goto :end
)
(
  echo map -r
  echo setvar RamLimitTo3GB -guid CD7CC258-31DB-22E6-9F22-63B0B8EED6B5 -bs -rt -nv =0x00000000
  echo rm \startup.nsh
  echo rm \EFI\BOOT\BOOTAA64.EFI
  echo mv \EFI\BOOT\BOOTAA64.WOR \EFI\BOOT\BOOTAA64.EFI
  echo \EFI\BOOT\BOOTAA64.EFI
) > "%bootDir%\startup.nsh"
call :RaiseEvent LogInfo, "Staged one-time UEFI Shell RAM unlock; the original EFI loader will be restored automatically."
goto :end

:: RaiseEvent Type, Data
:RaiseEvent
echo WoR-Event^|%~1^|%~2
goto :eof

:end
:: a non-zero exit code aborts the whole installation, and a missing answer file is not worth that
exit /b 0
