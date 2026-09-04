# Security Policy

## Supported versions

This fork keeps a single supported line. Fixes land on the default branch and in the
next version; there are no long-term support branches.

| Version                        | Supported                      |
| ------------------------------ | ------------------------------ |
| 1.0.x                          | Yes                            |
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
- **Privilege escalation through the sudo helpers.** The askpass scripts and the
  credential keep-alive run while the user's timestamp is live.
- **Command or argument injection** through a filename, drive label, environment
  variable or `config.txt` body that reaches a shell, `osascript`, or `yad`.
- **Secrets in the logs.** Failures keep `$DL_DIR/last-run.log`; it must never contain a
  password.

## What we do not consider a vulnerability

- **Needing root.** Flashing a disk requires it. That is the tool's purpose.
- **Downloading Windows from Microsoft.** This is done over HTTPS from Microsoft's own
  update servers via [uupdump][uupdump], and it is [legal][legality]. No proprietary
  material is redistributed here.
- **Erasing the drive you selected.** You are warned twice.
- **`VERIFY_TLS=0`.** It exists for hosts with a broken CA bundle, is documented as a
  downgrade, and is opt-in.
- **The self-updater.** It is off by default (`NO_UPDATE=1`), only fast-forwards a clean
  git checkout, and refuses to touch one with uncommitted changes. Report it if you can
  make it do something else.
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
