# Security Policy

## Supported versions

This fork keeps a single supported line. Fixes land on the default branch and in the
next version; there are no long-term support branches.

| Version                        | Supported                      |
| ------------------------------ | ------------------------------ |
| 2.1.x                          | Yes                            |
| 2.0.x                          | No                             |
| 1.0.x                          | No                             |
| Upstream `Botspot/wor-flasher` | Report to [upstream][upstream] |

Check what you are running with:

```bash
./install-wor.sh --version
```

## Reporting a vulnerability

**Please do not open a public issue for a security problem.**

Use [GitHub's private vulnerability reporting][report] on this repository, or contact
Blackout Secure at [blackoutsecure.app][bos].

Please include:

- The version (`./install-wor.sh --version`) and your host OS.
- What an attacker can do, and what they need in order to do it.
- Steps to reproduce, ideally with `DRY_RUN=1` so nothing is written to a drive.

You can expect an acknowledgement within a few days. We will tell you what we intend to
do and roughly when, and we will credit you in the fix unless you would rather we did
not.

If the problem also affects [Botspot/wor-flasher][upstream], please report it there too.
This fork cannot ship a fix for upstream's users.

## What we consider a vulnerability

This tool needs `sudo`, downloads several gigabytes over the network, and then erases a
whole disk. The interesting failure modes follow from that:

- **Writing to the wrong drive.** Anything that lets a target other than the one the
  user chose be erased, or that defeats `is_safe_target_device` and the host boot-disk
  guard.
- **Tampered downloads being accepted.** The WoR-PE installer is pinned by SHA-256, ESD
  images are checked against Microsoft's published SHA-1, and cached payloads carry a
  SHA-256 manifest. A way to get unverified content past any of those is a
  vulnerability.
- **IoT Core image substitution.** Native acquisition and FFU application use the
  reviewed digest-pinned image manifest. Package hashes, FFU chunk hashes and
  mandatory read-back checks cannot be disabled through desktop cache/TLS options.
  Catalog-signature interpretation is not the trust anchor: exact reviewed file
  digests are. A wildcard FFU platform ID does not authorize another board.
- **IoT Core target replacement.** Erase consent is bound to the selected device
  node, capacity and previewed partition scheme; disks backing the source, running installer and active log
  must not be targets. Failure to determine that relationship denies the write.
  Native device IDs may be signed on macOS; their exact values, inode and raw
  device identity remain bound through opening and writing.
- **IoT Core GPT cleanup.** Cleanup is part of the confirmed FFU write, after
  independent complete-source verification and target binding. GPT headers/table
  CRCs and bounded sector ranges must validate before any cleanup write, and cleared
  metadata outside the final image extents is included in mandatory read-back.
  An invalid layout or a changed source/target must not reach destructive cleanup.
- **Explicit IoT full-drive reset.** The opt-in full wipe needs a separate
  confirmation as well as normal erase consent. It runs only after image
  verification and all ordinary target protections. Every addressable sector
  is zeroed and read back before image application; I/O failures, device/source
  changes or interruptions stop the run. Unlike bounded GPT cleanup, it does
  not trust old partition metadata or follow its offsets. It does not bypass
  identity/root/source-disk guards, securely erase remapped flash blocks or
  repair physically failing storage.
- **IoT Core display overrides.** The unchanged, allowlisted FFU is verified before
  deriving an opt-in HDMI-only patch. Only the existing FAT16 boot config file
  and its directory size can change, within the existing allocation. Firmware
  boot keys, FAT tables and other files stay intact. Configured output blocks
  participate in the same mandatory read-back as every other final image block;
  invalid settings or boot geometry fail before target access.
- **IoT Core administrator setup.** This is an opt-in post-boot operation, not an
  offline account-database or FFU edit. The local device address and its SSH key
  require explicit confirmation before credentials are sent. SSH host-key checking
  stays strict, and a fresh login must verify the desired name and the same local
  built-in administrator SID before success. DefaultAccount/service accounts are
  excluded. Passwords use masked controls, private temporary state and SSH stdin,
  not host command arguments, summaries or logs. Failed/partial updates remain
  unverified; never silently retry them under the old credentials.
- **Automatic IoT address lookup.** Automatic mode resolves only the documented
  default IoT hostname after Connect. It does not scan subnets or alter the Pi's
  network configuration. Non-local or ambiguous automatic results stop before
  SSH credentials are sent; the selected endpoint's SSH identity still needs
  explicit confirmation and remains pinned during personalization.
- **IoT Core language selection.** The language preference uses the same pinned
  SSH connection, but the actual IoTSettings operation must run as DefaultAccount,
  never the administrator. The selected tag must be installed, the temporary task
  uses the existing interactive token without stored credentials or elevated
  run level, and task/files cleanup failures block a verified result. Accepted
  but inactive settings are pending-reboot, not verified. No account membership,
  SSH key authorization, image trust rule or language-pack source is widened.
- **Privilege escalation through the sudo helpers.** The askpass scripts and the
  credential keep-alive run while the user's timestamp is live.
- **Command or argument injection** through a filename, drive label, environment
  variable or `config.txt` body that reaches a shell, `osascript`, or `yad`.
- **Secrets in the logs.** Failures keep timestamped logs under `$DL_DIR/logs/`, refresh
  `$DL_DIR/last-run.log`, or write wherever `WOR_LOG_FILE` points; they must never contain a
  password.

## What we do not consider a vulnerability

- **Needing root.** Flashing a disk requires it. That is the tool's purpose.
- **Downloading Windows from Microsoft.** This is done over HTTPS from Microsoft's own
  update servers via [uupdump][uupdump], and it is [legal][legality]. No proprietary
  material is redistributed here.
- **Erasing the drive you selected.** You are warned twice.
- **`VERIFY_TLS=0`.** It exists for hosts with a broken CA bundle, is documented as a
  downgrade, and is opt-in.
- **The release check.** It is enabled by default, makes one read-only HTTPS request to the
  GitHub releases API, and only prints a notice. It never writes to disk, never executes
  downloaded content, and never modifies the installation. It can be disabled with
  `CHECK_FOR_UPDATES=0` or the legacy `NO_UPDATE=1`. Report it if you can make it do
  anything beyond reporting a version.
- Vulnerabilities in Windows itself, in the WoR PE installer, or in the Pi UEFI
  firmware. Report those to [worproject.com][wor] and [pftf][pftf] respectively.

## Disclosure

We ask for coordinated disclosure. Give us a reasonable window to ship a fix — 90 days
is the default, shorter if the issue is being exploited. We will not take legal action
against anyone who reports in good faith and does not exfiltrate data, degrade a service
or access an account that is not theirs.

[upstream]: https://github.com/Botspot/wor-flasher/issues
[report]: https://github.com/blackoutsecure/wor-flasher/security/advisories/new
[bos]: https://blackoutsecure.app
[uupdump]: https://uupdump.net
[legality]: https://www.raspberrypi.org/forums/viewtopic.php?f=29&t=318599
[wor]: https://worproject.com/contact
[pftf]: https://github.com/pftf
