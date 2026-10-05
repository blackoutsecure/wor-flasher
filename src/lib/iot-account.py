#!/usr/bin/env python3
"""Post-boot IoT Core account/language personalization over fingerprint-pinned SSH.

Only the built-in administrator (SID ending in 500) on Windows 10 IoT Core is
changed. Language settings run under DefaultAccount without changing that account.
The allowlisted FFU and disk are never accessed.
Secrets arrive through stdin, not arguments, and are sent over SSH stdin.
The current password is kept only in an owner-private temporary askpass file.
No success is reported until a new SSH login verifies the account name and SID.
"""
from __future__ import annotations

import argparse
import base64
import getpass
import hashlib
import ipaddress
import json
import os
import re
import shlex
import shutil
import socket
import stat
import struct
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any, Optional, Sequence


MAX_INPUT = 32768
MARKER = "WOR_IOT_ACCOUNT_JSON:"
LOCALE_MARKER = "WOR_IOT_LOCALE_JSON:"
SCRIPT = Path(__file__).with_suffix(".ps1")
KEY_TYPES = ("ssh-ed25519", "ecdsa-sha2-nistp256", "ssh-rsa")


class AccountError(ValueError):
    pass


def normalize_language(value: str) -> str:
    if type(value) is not str or not re.fullmatch(r"[A-Za-z]{2,3}(?:-[A-Za-z0-9]{2,8}){1,2}", value):
        raise AccountError("Choose a Windows IoT language tag, for example en-US or fr-FR.")
    parts = value.split("-")
    return "-".join([parts[0].lower()] + [part.title() if len(part) == 4 else part.upper() for part in parts[1:]])


def account_preferences(data: dict[str, Any], *, metadata_only: bool = False) -> dict[str, object]:
    enabled = data.get("accountSetup", False)
    username = data.get("accountUsername", "Administrator")
    password = data.get("accountPassword", "")
    if type(enabled) is not bool or not isinstance(username, str) or not isinstance(password, str):
        raise AccountError("Invalid IoT account preferences.")
    if enabled:
        validate_username(username)
        if not metadata_only:
            validate_password(password, new=True)
    elif password:
        validate_password(password, new=False)
    language_setup = data.get("languageSetup", False)
    language = data.get("language", "en-US")
    if type(language_setup) is not bool or not isinstance(language, str):
        raise AccountError("Invalid IoT language preferences.")
    language = normalize_language(language)
    return {"accountSetup": enabled, "accountUsername": username, "accountPassword": password,
            "languageSetup": language_setup, "language": language}


def validate_username(value: str) -> None:
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]{0,19}", value):
        raise AccountError("Use a 1-20 character IoT username containing letters, digits, dot, underscore or hyphen.")
    if value.casefold() in ("defaultaccount", "guest", "system", "wdagutilityaccount"):
        raise AccountError("Choose an administrator username, not a reserved system account.")


def validate_password(value: str, *, new: bool) -> None:
    length = len(value.encode("utf-16-le")) // 2
    if not (12 if new else 1) <= length <= 127 or any(ord(char) < 32 for char in value):
        raise AccountError("The new IoT password must contain 12-127 characters and no control characters." if new
                           else "Enter the current IoT password without control characters.")


def read_json() -> dict[str, Any]:
    raw = sys.stdin.buffer.read(MAX_INPUT + 1)
    if len(raw) > MAX_INPUT:
        raise AccountError("IoT account request is too large.")
    try:
        data: dict[str, Any] = json.loads(raw)
    except (ValueError, UnicodeDecodeError) as exc:
        raise AccountError("Invalid IoT account request.") from exc
    if type(data) is not dict:
        raise AccountError("IoT account request must be an object.")
    return data


def local_address(host: str) -> str:
    if type(host) is not str or len(host) > 253 or not re.fullmatch(r"[A-Za-z0-9_.:%-]+", host):
        raise AccountError("Enter the Pi's local IP address or hostname, not a URL.")
    try:
        addresses = socket.getaddrinfo(host, 22, type=socket.SOCK_STREAM)
    except socket.gaierror as exc:
        raise AccountError("Cannot resolve the Pi's local address.") from exc
    candidates: list[str] = []
    for entry in addresses:
        numeric = entry[4][0]
        if not isinstance(numeric, str):
            raise AccountError("The Pi address returned invalid network address data.")
        address = ipaddress.ip_address(numeric.split("%", 1)[0])
        allowed = address.is_private or address.is_link_local
        if not allowed or address.is_loopback or address.is_unspecified or address.is_multicast:
            raise AccountError("IoT account setup is limited to a device on your private local network.")
        if numeric not in candidates:
            candidates.append(numeric)
    if not candidates:
        raise AccountError("The Pi address has no usable local network address.")
    return candidates[0]


def key_fingerprint(key_type: str, key: str) -> str:
    if type(key_type) is not str or key_type not in KEY_TYPES or type(key) is not str or len(key) > 16384:
        raise AccountError("Invalid SSH host key.")
    try:
        raw = base64.b64decode(key, validate=True)
    except ValueError as exc:
        raise AccountError("Invalid SSH host key.") from exc
    if len(raw) < 4:
        raise AccountError("Invalid SSH host key.")
    length = struct.unpack_from(">I", raw)[0]
    if raw[4:4 + length] != key_type.encode("ascii"):
        raise AccountError("SSH host key type does not match its key data.")
    return "SHA256:" + base64.b64encode(hashlib.sha256(raw).digest()).decode("ascii").rstrip("=")


def probe_device(host: str) -> dict[str, object]:
    address = local_address(host)
    binary = shutil.which("ssh-keyscan")
    if binary is None:
        raise AccountError("OpenSSH client tools are required for post-boot IoT account setup.")
    try:
        response = subprocess.run([binary, "-T", "5", "-p", "22", "-t", "ed25519,ecdsa,rsa", address],
                                  capture_output=True, timeout=15, check=False)
    except (OSError, subprocess.TimeoutExpired) as exc:
        raise AccountError("Cannot read the Pi's SSH identity. Boot it and check its address.") from exc
    if response.returncode != 0 or len(response.stdout) > 65536:
        raise AccountError("The Pi's SSH service is unavailable or returned invalid identity data.")
    keys: dict[str, str] = {}
    for line in response.stdout.decode("ascii", errors="strict").splitlines():
        if line.startswith("#"):
            continue
        fields = line.split()
        if len(fields) == 3 and fields[1] in KEY_TYPES:
            key_fingerprint(fields[1], fields[2])
            if fields[1] in keys and keys[fields[1]] != fields[2]:
                raise AccountError("The SSH scan returned conflicting device identities.")
            keys[fields[1]] = fields[2]
    for key_type in KEY_TYPES:
        if key_type in keys:
            key = keys[key_type]
            return {"address": address, "keyType": key_type, "key": key,
                    "fingerprint": key_fingerprint(key_type, key)}
    raise AccountError("The Pi did not return a supported SSH host key.")


def remote_script(operation: str, username: str, password: str = "") -> str:
    payload = base64.b64encode(json.dumps(
        {"operation": operation, "username": username, "password": password},
        ensure_ascii=True,
    ).encode("utf-8")).decode("ascii")
    template = SCRIPT.read_text(encoding="utf-8")
    if template.count("__WOR_REQUEST__") != 1:
        raise AccountError("IoT account management script is incomplete.")
    return template.replace("__WOR_REQUEST__", payload) + "\n\n"


def locale_script(language: str) -> str:
    language = normalize_language(language)
    template = Path(__file__).with_name("iot-locale.ps1").read_text(encoding="utf-8")
    user_script = Path(__file__).with_name("iot-locale-user.ps1").read_bytes()
    payload = base64.b64encode(json.dumps({"language": language}).encode()).decode("ascii")
    if template.count("__WOR_REQUEST__") != 1 or template.count("__WOR_USER_SCRIPT__") != 1:
        raise AccountError("IoT language management script is incomplete.")
    return template.replace("__WOR_REQUEST__", payload).replace(
        "__WOR_USER_SCRIPT__", base64.b64encode(user_script).decode("ascii")
    ) + "\n\n"


def parse_locale_response(output: bytes) -> dict[str, Any]:
    if len(output) > 1024 * 1024:
        raise AccountError("The Pi returned excessive language configuration output.")
    cleaned = re.sub(rb"\x1b\[[0-?]*[ -/]*[@-~]", b"", output).decode("utf-8", errors="replace")
    records = [line[len(LOCALE_MARKER):] for line in cleaned.splitlines() if line.startswith(LOCALE_MARKER)]
    if len(records) != 1:
        raise AccountError("No unambiguous IoT language result was received; the selected language is not verified.")
    try:
        result: dict[str, Any] = json.loads(records[0])
    except ValueError as exc:
        raise AccountError("Invalid IoT language result.") from exc
    if (
        type(result) is not dict or result.get("state") not in ("verified", "pending-reboot", "unsupported", "failed")
        or type(result.get("language")) is not str or type(result.get("changed")) is not bool
        or result.get("stage") not in ("device-guard", "default-account-task", "default-account-guard", "language-support",
                                     "language-update", "language-verification", "task-cleanup")
        or type(result.get("available")) is not list
        or not all(isinstance(value, str) for value in result["available"])
    ):
        raise AccountError("The Pi returned incomplete IoT language state.")
    normalize_language(result["language"])
    return result


def parse_response(output: bytes) -> dict[str, Any]:
    if len(output) > 1024 * 1024:
        raise AccountError("The Pi returned excessive account-setup output; its account state is uncertain.")
    cleaned = re.sub(rb"\x1b\[[0-?]*[ -/]*[@-~]", b"", output)
    responses = [line[len(MARKER):] for line in cleaned.decode("utf-8", errors="replace").splitlines()
                 if line.startswith(MARKER)]
    if len(responses) != 1:
        raise AccountError("No unambiguous account result was received; the credentials may already have changed.")
    try:
        response: dict[str, Any] = json.loads(responses[0])
    except ValueError as exc:
        raise AccountError("The Pi returned an invalid account result; its account state is uncertain.") from exc
    if type(response) is not dict or response.get("state") not in ("applied", "verified", "failed"):
        raise AccountError("The Pi returned an invalid account state.")
    if (
        type(response.get("username")) is not str or type(response.get("sid")) is not str
        or type(response.get("code")) is not int or not 0 <= response["code"] <= 2**32 - 1
        or type(response.get("passwordChanged")) is not bool or type(response.get("usernameChanged")) is not bool
        or response.get("stage") not in ("device-guard", "native-preflight", "password-update", "username-update", "login-verification")
    ):
        raise AccountError("The Pi returned incomplete account state; credentials may already have changed.")
    return response


def ssh_operation(
    binary: str, directory: Path, address: str, username: str, password: str,
    script: str, *, locale: bool = False,
) -> dict[str, Any]:
    secret = directory / "current-password"
    secret.write_text(password, encoding="utf-8")
    secret.chmod(0o600)
    askpass = directory / "askpass"
    askpass.write_text("#!/bin/bash\nset -euo pipefail\nexec " +
                       shlex.quote(sys.executable) + " " + shlex.quote(str(Path(__file__).resolve())) +
                       ' askpass "$@"\n', encoding="utf-8")
    askpass.chmod(0o700)
    environment = dict(os.environ, SSH_ASKPASS=str(askpass), SSH_ASKPASS_REQUIRE="force",
                       WOR_IOT_ASKPASS_FILE=str(secret), DISPLAY=os.environ.get("DISPLAY", "WoR-Flasher"))
    arguments = [
        binary, "-T", "-o", "StrictHostKeyChecking=yes", "-o", "HostKeyAlias=wor-iot-paired",
        "-o", f"UserKnownHostsFile={directory / 'known_hosts'}", "-o", "GlobalKnownHostsFile=/dev/null",
        "-o", "PreferredAuthentications=password", "-o", "PubkeyAuthentication=no",
        "-o", "NumberOfPasswordPrompts=1", "-o", "ConnectTimeout=10",
        "-o", "ConnectionAttempts=1", "--", f"{username}@{address}",
        "powershell.exe -NoLogo -NoProfile -NonInteractive -Command -",
    ]
    try:
        response = subprocess.run(arguments, input=script.encode("utf-8"), capture_output=True,
                                  timeout=60, env=environment, start_new_session=True, check=False)
    except (OSError, subprocess.TimeoutExpired) as exc:
        raise AccountError("SSH account setup failed or timed out; verify the old/new login before retrying.") from exc
    finally:
        secret.unlink(missing_ok=True)
    if response.returncode != 0:
        raise AccountError("SSH login or account setup failed. Check the address, trusted key and current credentials; an update may already have occurred.")
    return parse_locale_response(response.stdout) if locale else parse_response(response.stdout)


def configure_account(request: dict[str, Any]) -> dict[str, object]:
    pairing: dict[str, Any] = request.get("pairing", {})
    current = request.get("currentUsername", "Administrator")
    current_password = request.get("currentPassword", "")
    account_setup = request.get("accountSetup", True)
    language_setup = request.get("languageSetup", False)
    if type(account_setup) is not bool or type(language_setup) is not bool or not (account_setup or language_setup):
        raise AccountError("Select an IoT account or language preference to apply.")
    username, password = request.get("accountUsername", current), request.get("accountPassword", "")
    if not isinstance(current, str) or not isinstance(current_password, str) or not isinstance(username, str) or not isinstance(password, str):
        raise AccountError("Invalid IoT account setup credentials.")
    validate_username(current)
    validate_username(username)
    validate_password(current_password, new=False)
    if account_setup:
        validate_password(password, new=True)
    language = normalize_language(request.get("language", "en-US"))
    if type(pairing) is not dict or request.get("confirmIdentity") is not True:
        raise AccountError("Confirm the Pi's SSH identity before sending credentials.")
    address = pairing.get("address")
    if not isinstance(address, str) or local_address(address) != address:
        raise AccountError("The paired device address changed.")
    key_type, key = pairing.get("keyType"), pairing.get("key")
    if not isinstance(key_type, str) or not isinstance(key, str):
        raise AccountError("Invalid paired SSH host key.")
    if key_fingerprint(key_type, key) != pairing.get("fingerprint"):
        raise AccountError("The paired SSH identity is inconsistent.")
    binary = shutil.which("ssh")
    if binary is None:
        raise AccountError("OpenSSH is required for post-boot IoT account setup.")
    with tempfile.TemporaryDirectory(prefix="wor-iot-account-") as temporary:
        directory = Path(temporary)
        known = directory / "known_hosts"
        known.write_text(f"wor-iot-paired {key_type} {key}\n", encoding="ascii")
        known.chmod(0o600)
        language_result: dict[str, Any] = {"state": "not-assessed", "language": language}
        if language_setup:
            language_result = ssh_operation(
                binary, directory, address, current, current_password, locale_script(language), locale=True
            )
            if language_result["language"].casefold() != language.casefold():
                raise AccountError("The Pi returned a different language than selected.")
            if language_result["state"] == "unsupported":
                raise AccountError(f"The selected IoT language {language} is not installed. No administrator credentials were changed; choose an installed language.")
            if language_result["state"] not in ("verified", "pending-reboot"):
                raise AccountError("IoT language configuration failed or its cleanup could not be verified. Administrator credentials were not changed.")
        if not account_setup:
            return {"state": language_result["state"], "username": current, "address": address,
                    "accountState": "not-assessed", "languageState": language_result["state"], "language": language}
        result = ssh_operation(binary, directory, address, current, current_password,
                               remote_script("apply", username, password))
        if result["state"] != "applied":
            changed = result.get("passwordChanged") is True or result.get("usernameChanged") is True
            raise AccountError(
                f"IoT account setup failed (Windows status {result.get('code', 0)}; stage {result.get('stage', 'not reported')}). " +
                ("A partial change occurred; check the new password with the original/desired username."
                 if changed else "No completed account update was reported.")
            )
        if result.get("username", "").casefold() != username.casefold() or not re.fullmatch(r"S-1-[0-9-]+-500", result.get("sid", "")):
            raise AccountError("The Pi returned an unexpected administrator identity; account verification failed.")
        verified = ssh_operation(binary, directory, address, username, password, remote_script("verify", username))
        if verified.get("state") != "verified" or verified.get("sid") != result["sid"] or verified.get("username", "").casefold() != username.casefold():
            raise AccountError("The new IoT login could not be verified. Credentials may have changed; do not assume setup completed.")
        return {"state": "pending-reboot" if language_result["state"] == "pending-reboot" else "verified",
                "username": username, "sid": result["sid"], "address": address,
                "accountState": "verified", "languageState": language_result["state"], "language": language}


def askpass() -> int:
    path = Path(os.environ.get("WOR_IOT_ASKPASS_FILE", ""))
    if len(sys.argv) < 3 or "password" not in sys.argv[2].lower():
        return 1
    information = path.lstat()
    if not stat.S_ISREG(information.st_mode) or information.st_uid != os.getuid() or information.st_mode & 0o077:
        return 1
    password = path.read_text(encoding="utf-8")
    validate_password(password, new=False)
    print(password)
    return 0


def main(argv: Optional[Sequence[str]] = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("preferences", "probe", "configure", "askpass"))
    parser.add_argument("prompt", nargs="?")
    parser.add_argument("--interactive", action="store_true")
    parser.add_argument("--metadata-only", action="store_true")
    parser.add_argument("--language", help="requested installed IoT UI language for interactive setup")
    parser.add_argument("--language-only", action="store_true")
    args = parser.parse_args(argv)
    try:
        if args.command == "askpass":
            return askpass()
        if args.interactive:
            if args.command != "configure":
                raise AccountError("Interactive mode is only for post-boot account configuration.")
            pairing = probe_device(input("Pi local address: ").strip())
            print(f"Resolved device: {pairing['address']}\nSSH identity: {pairing['fingerprint']}")
            if input("Verify this is your Pi; type CONFIRM to continue: ").strip() != "CONFIRM":
                raise AccountError("Device identity was not confirmed; no credentials were sent.")
            request: dict[str, Any] = {
                "pairing": pairing, "confirmIdentity": True,
                "currentUsername": input("Current IoT administrator [Administrator]: ").strip() or "Administrator",
                "currentPassword": getpass.getpass("Current IoT password: "),
                "accountSetup": not args.language_only,
                "accountUsername": "Administrator" if args.language_only else (input("New IoT administrator [Administrator]: ").strip() or "Administrator"),
                "accountPassword": "" if args.language_only else getpass.getpass("New IoT password (12-127 characters): "),
                "languageSetup": args.language is not None,
                "language": args.language or "en-US",
            }
        else:
            request = read_json()
        if args.command == "preferences":
            result = account_preferences(request, metadata_only=args.metadata_only)
        elif args.command == "probe":
            result = probe_device(request.get("host", ""))
        else:
            result = configure_account(request)
        print(json.dumps(result, ensure_ascii=True))
        return 0
    except (AccountError, OSError, UnicodeError) as exc:
        message = str(exc) if isinstance(exc, AccountError) else "IoT account setup encountered an I/O or encoding error."
        print(f"IoT account setup: {message}", file=sys.stderr)
        return 1
    except KeyboardInterrupt:
        print("IoT account setup interrupted; credentials may have changed. Verify the old/new login before retrying.", file=sys.stderr)
        return 130


if __name__ == "__main__":
    raise SystemExit(main())
