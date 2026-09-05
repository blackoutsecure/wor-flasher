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

if not exist "%ramUnlockSource%" goto :end

if not exist "%scriptsDir%\" mkdir "%scriptsDir%"

copy /y "%ramUnlockSource%" "%scriptsDir%\Pi4Disable3GB.ps1" >nul
if errorlevel 1 (
  call :RaiseEvent LogWarn, "Could not install the Pi 4 RAM unlock; the 3 GB limit will stay enabled."
  goto :end
)

call :RaiseEvent LogInfo, "Pi 4 RAM unlock installed to %scriptsDir%\Pi4Disable3GB.ps1"
goto :end

:: RaiseEvent Type, Data
:RaiseEvent
echo WoR-Event^|%~1^|%~2
goto :eof

:end
:: a non-zero exit code aborts the whole installation, and a missing answer file is not worth that
exit /b 0
