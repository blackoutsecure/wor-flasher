#!/usr/bin/env python3
"""Offline IoT administrator contracts; fixture credentials are never real logins."""
from __future__ import annotations

import base64
import importlib.util
import json
import os
import socket
import struct
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("iot_account", ROOT / "src/lib/iot-account.py")
account = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = account
SPEC.loader.exec_module(account)
CURRENT = "Fixture-current-123"
DESIRED = 'Fixture-new-"%&-456'
KEY_TYPE = "ssh-ed25519"
KEY = base64.b64encode(struct.pack(">I", len(KEY_TYPE)) + KEY_TYPE.encode() + b"\0" * 32).decode()
SID = "S-1-5-21-1-2-3-500"
ADDRESS = "192.168.50.23"


def result(state: str, username: str = "FixtureAdmin", *, changed: bool = False, code: int = 0) -> bytes:
    value = {"state": state, "username": username, "sid": SID, "passwordChanged": changed,
             "usernameChanged": changed, "code": code, "stage": "login-verification"}
    return (account.MARKER + json.dumps(value) + "\n").encode()

def locale_result(state: str, language: str = "fr-FR") -> bytes:
    value = {"state": state, "language": language, "changed": state in ("verified", "pending-reboot"),
             "available": ["en-US", "fr-FR"], "stage": "language-verification"}
    return (account.LOCALE_MARKER + json.dumps(value) + "\n").encode()

@unittest.skipUnless(os.environ.get("WOR_TEST_PWSH"), "PowerShell runtime validator not supplied")
class LocaleRuntimeTests(unittest.TestCase):
    def test_default_user_language_lifecycle_with_fixture_tool(self) -> None:
        original = (ROOT / "src/lib/iot-locale-user.ps1").read_text()
        cases = (
            ("already", "fr-FR", "en-US fr-FR", "verified", False, "503"),
            ("apply", "en-US", "en-US fr-FR", "verified", True, "503"),
            ("restart", "en-US", "en-US fr-FR", "pending-reboot", True, "503"),
            ("missing-pack", "en-US", "en-US", "unsupported", False, "503"),
            ("administrator", "en-US", "en-US fr-FR", "failed", False, "500"),
        )
        for name, initial, installed, expected, changed, rid in cases:
            with self.subTest(name=name), tempfile.TemporaryDirectory(prefix="iot-language-fixture-") as temporary:
                directory = Path(temporary)
                (directory / "System32").mkdir()
                setting = directory / "current.txt"
                setting.write_text(initial)
                tool = directory / "System32/IoTSettings.exe"
                tool.write_text(
                    "#!/bin/bash\nset -euo pipefail\ncase \"$1\" in\n"
                    f"list) printf '%s\\n' '{installed}';;\n"
                    f"get) cat '{setting}';;\n"
                    + ("set) :;;\n" if name == "restart" else f"set) printf '%s' \"$3\" > '{setting}';;\n")
                    + "*) exit 91;;\nesac\n"
                )
                tool.chmod(0o700)
                result_path = directory / "result.json"
                payload = base64.b64encode(json.dumps({"language": "fr-FR", "resultPath": str(result_path)}).encode()).decode()
                script = original.replace(
                    "[Security.Principal.WindowsIdentity]::GetCurrent().User.Value", f"'S-1-5-21-1-2-3-{rid}'"
                ).replace("__WOR_LOCALE_USER_REQUEST__", payload)
                source = directory / "run.ps1"
                source.write_text(script)
                response = subprocess.run(
                    [os.environ["WOR_TEST_PWSH"], "-NoProfile", "-NonInteractive", "-File", str(source)],
                    capture_output=True, timeout=30, env=dict(os.environ, SystemRoot=str(directory)), check=False,
                )
                self.assertEqual(response.returncode, 0, response.stderr)
                result_data = json.loads(result_path.read_text())
                self.assertEqual(result_data["state"], expected)
                self.assertEqual(result_data["changed"], changed)


class AccountTests(unittest.TestCase):
    def setUp(self) -> None:
        self.addCleanup(mock.patch.stopall)
        mock.patch.object(account.socket, "getaddrinfo", return_value=[
            (socket.AF_INET, socket.SOCK_STREAM, 0, "", (ADDRESS, 22))
        ]).start()
        mock.patch.object(account.shutil, "which", side_effect=lambda name: "/fixture/" + name).start()
        self.pairing = {"address": ADDRESS, "keyType": KEY_TYPE, "key": KEY,
                        "fingerprint": account.key_fingerprint(KEY_TYPE, KEY)}
        self.request = {"pairing": self.pairing, "confirmIdentity": True, "currentUsername": "Administrator",
                        "currentPassword": CURRENT, "accountUsername": "FixtureAdmin", "accountPassword": DESIRED}

    def test_preferences_require_explicit_setup_and_strong_password(self) -> None:
        self.assertEqual(account.account_preferences({})["accountSetup"], False)
        self.assertTrue(account.account_preferences(
            {"accountSetup": True, "accountUsername": "FixtureAdmin", "accountPassword": DESIRED}
        )["accountSetup"])
        for bad in ("short", "", "Fixture\npassword123"):
            with self.assertRaises(account.AccountError):
                account.account_preferences({"accountSetup": True, "accountUsername": "FixtureAdmin", "accountPassword": bad})
        for bad in ("DefaultAccount", "Guest", "bad&name", "../Administrator", "x" * 21):
            with self.assertRaises(account.AccountError):
                account.validate_username(bad)

    def test_probe_returns_identity_without_sending_credentials(self) -> None:
        response = subprocess.CompletedProcess([], 0, f"{ADDRESS} {KEY_TYPE} {KEY}\n".encode(), b"")
        with mock.patch.object(account.subprocess, "run", return_value=response) as run:
            pairing = account.probe_device(ADDRESS)
        self.assertEqual(pairing, self.pairing)
        self.assertEqual(run.call_args.args[0][0], "/fixture/ssh-keyscan")
        self.assertNotIn("input", run.call_args.kwargs)

    def test_remote_public_address_and_changed_pairing_fail_before_ssh(self) -> None:
        for address in ("8.8.8.8", "127.0.0.1", "0.0.0.0", "224.1.2.3"):
            with mock.patch.object(account.socket, "getaddrinfo", return_value=[
                (socket.AF_INET, socket.SOCK_STREAM, 0, "", (address, 22))
            ]), mock.patch.object(account.subprocess, "run") as run:
                with self.assertRaises(account.AccountError):
                    account.probe_device(address)
                self.assertEqual(run.call_count, 0)
        for change in ({"confirmIdentity": False}, {"pairing": dict(self.pairing, fingerprint="wrong")}):
            with mock.patch.object(account.subprocess, "run") as run:
                with self.assertRaises(account.AccountError):
                    account.configure_account(dict(self.request, **change))
                self.assertEqual(run.call_count, 0)

    def test_update_is_verified_by_a_second_pinned_login_without_secret_arguments(self) -> None:
        calls: list[tuple[list[str], str]] = []
        private_files: list[Path] = []

        def ssh(arguments: list[str], **kwargs: object) -> subprocess.CompletedProcess[bytes]:
            environment = kwargs["env"]
            secret = Path(environment["WOR_IOT_ASKPASS_FILE"])
            private_files.append(secret)
            self.assertEqual(secret.stat().st_mode & 0o777, 0o600)
            password = secret.read_text()
            self.assertIn(password, (CURRENT, DESIRED))
            self.assertNotIn(password, " ".join(arguments))
            self.assertIn("StrictHostKeyChecking=yes", arguments)
            self.assertIn("HostKeyAlias=wor-iot-paired", arguments)
            self.assertEqual(environment["SSH_ASKPASS_REQUIRE"], "force")
            script = kwargs["input"].decode()
            self.assertNotIn(DESIRED, script)
            self.assertIn("NetUserSetInfo", script)
            self.assertIn("-500$", script)
            calls.append((arguments, password))
            return subprocess.CompletedProcess(arguments, 0, result("applied" if len(calls) == 1 else "verified", changed=len(calls) == 1), b"")

        with mock.patch.object(account.subprocess, "run", side_effect=ssh):
            outcome = account.configure_account(self.request)
        self.assertEqual(outcome["state"], "verified")
        self.assertEqual(outcome["sid"], SID)
        self.assertEqual([call[1] for call in calls], [CURRENT, DESIRED])
        self.assertIn("Administrator@" + ADDRESS, calls[0][0])
        self.assertIn("FixtureAdmin@" + ADDRESS, calls[1][0])
        self.assertTrue(all(not path.exists() for path in private_files))

    def test_rename_and_password_failures_never_report_success(self) -> None:
        for response in (
            result("failed", "Administrator", code=2245),
            result("failed", "Administrator", changed=True, code=2224),
            result("applied", "AnotherAccount", changed=True),
            result("applied", changed=True).replace(SID.encode(), b"S-1-5-21-1-2-3-501"),
        ):
            with mock.patch.object(account.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, response, b"")):
                with self.assertRaises(account.AccountError):
                    account.configure_account(self.request)

    def test_failed_reconnect_leaves_account_state_explicitly_unverified(self) -> None:
        responses = [
            subprocess.CompletedProcess([], 0, result("applied", changed=True), b""),
            subprocess.CompletedProcess([], 255, b"", DESIRED.encode()),
        ]
        with mock.patch.object(account.subprocess, "run", side_effect=responses):
            with self.assertRaises(account.AccountError) as error:
                account.configure_account(self.request)
        self.assertNotIn(DESIRED, str(error.exception))
        self.assertIn("may already", str(error.exception))

    def test_different_sid_on_reconnect_is_not_verified(self) -> None:
        responses = [
            subprocess.CompletedProcess([], 0, result("applied", changed=True), b""),
            subprocess.CompletedProcess([], 0, result("verified").replace(SID.encode(), b"S-1-5-21-4-5-6-500"), b""),
        ]
        with mock.patch.object(account.subprocess, "run", side_effect=responses):
            with self.assertRaisesRegex(account.AccountError, "could not be verified"):
                account.configure_account(self.request)

    def test_password_payload_is_encoded_data_not_powershell_code(self) -> None:
        script = account.remote_script("apply", "FixtureAdmin", DESIRED)
        marker = script.split("FromBase64String('", 1)[1].split("')", 1)[0]
        data = json.loads(base64.b64decode(marker))
        self.assertEqual(data["password"], DESIRED)
        self.assertNotIn(DESIRED, script)
        self.assertEqual(data["username"], "FixtureAdmin")
        self.assertTrue(script.endswith("\n\n"))

    def test_missing_or_ambiguous_remote_result_is_not_success(self) -> None:
        for response in (b"", b"arbitrary output", result("verified") * 2, b"WOR_IOT_ACCOUNT_JSON:{}\n"):
            with self.assertRaises(account.AccountError):
                account.parse_response(response)
        self.assertEqual(account.parse_response(b"\x1b[?1h\x1b[?1l" + result("verified"))["state"], "verified")

    def test_askpass_checks_prompt_and_private_file_permissions(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            secret = Path(directory) / "secret"
            secret.write_text(CURRENT)
            secret.chmod(0o600)
            with mock.patch.dict(os.environ, WOR_IOT_ASKPASS_FILE=str(secret)), mock.patch.object(sys, "argv", ["helper", "askpass", "User password:"]), mock.patch("builtins.print") as output:
                self.assertEqual(account.askpass(), 0)
                output.assert_called_once_with(CURRENT)
            secret.chmod(0o644)
            with mock.patch.dict(os.environ, WOR_IOT_ASKPASS_FILE=str(secret)), mock.patch.object(sys, "argv", ["helper", "askpass", "User password:"]):
                self.assertEqual(account.askpass(), 1)
            with mock.patch.dict(os.environ, WOR_IOT_ASKPASS_FILE=str(secret)), mock.patch.object(sys, "argv", ["helper", "askpass", "Accept host key?"]):
                self.assertEqual(account.askpass(), 1)

    def test_language_only_does_not_change_administrator_credentials(self) -> None:
        request = dict(self.request, accountSetup=False, accountPassword="", languageSetup=True, language="fr-fr")
        with mock.patch.object(account.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, locale_result("verified"), b"")) as run:
            observed = account.configure_account(request)
        self.assertEqual(observed["language"], "fr-FR")
        self.assertEqual(observed["languageState"], "verified")
        self.assertEqual(observed["accountState"], "not-assessed")
        self.assertEqual(run.call_count, 1)
        script = run.call_args.kwargs["input"].decode()
        self.assertNotIn("NetUserSetInfo", script)
        self.assertIn("WOR_IOT_LOCALE_JSON:", script)
        self.assertIn("LogonType = 3", script)
        self.assertIn("RunLevel = 0", script)
        self.assertIn("DeleteTask", script)
        user_payload = script.split("FromBase64String('", 2)[2].split("')", 1)[0]
        self.assertIn("IoTSettings.exe", base64.b64decode(user_payload).decode())
        self.assertIn("-503$", base64.b64decode(user_payload).decode())

    def test_unsupported_language_blocks_account_changes(self) -> None:
        request = dict(self.request, languageSetup=True, language="fr-FR")
        with mock.patch.object(account.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, locale_result("unsupported"), b"")) as run:
            with self.assertRaisesRegex(account.AccountError, "not installed"):
                account.configure_account(request)
        self.assertEqual(run.call_count, 1)

    def test_language_preflight_and_verification_precede_any_account_mutation(self) -> None:
        request = dict(self.request, languageSetup=True, language="fr-FR")
        responses = [
            subprocess.CompletedProcess([], 0, locale_result("verified"), b""),
            subprocess.CompletedProcess([], 0, result("applied", changed=True), b""),
            subprocess.CompletedProcess([], 0, result("verified"), b""),
        ]
        with mock.patch.object(account.subprocess, "run", side_effect=responses) as run:
            observed = account.configure_account(request)
        self.assertEqual(observed["state"], "verified")
        self.assertEqual(observed["languageState"], "verified")
        self.assertEqual(observed["accountState"], "verified")
        self.assertIn("WOR_IOT_LOCALE_JSON:", run.call_args_list[0].kwargs["input"].decode())
        self.assertIn("NetUserSetInfo", run.call_args_list[1].kwargs["input"].decode())

    def test_accepted_but_inactive_language_is_pending_reboot_not_success(self) -> None:
        request = dict(self.request, accountSetup=False, languageSetup=True, language="fr-FR")
        with mock.patch.object(account.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, locale_result("pending-reboot"), b"")):
            observed = account.configure_account(request)
        self.assertEqual(observed["state"], "pending-reboot")
        self.assertEqual(observed["languageState"], "pending-reboot")

    def test_language_failures_or_wrong_result_never_report_verified(self) -> None:
        for response in (locale_result("failed"), locale_result("verified", "de-DE"), b"WOR_IOT_LOCALE_JSON:{}\n", b""):
            request = dict(self.request, accountSetup=False, languageSetup=True, language="fr-FR")
            with mock.patch.object(account.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, response, b"")):
                with self.assertRaises(account.AccountError):
                    account.configure_account(request)
        for tag in ("fr-FR; invoke-evil", "../en-US", "", "a", "en_US"):
            with self.assertRaises(account.AccountError):
                account.normalize_language(tag)


if __name__ == "__main__":
    unittest.main(verbosity=2)
