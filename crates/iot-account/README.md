# Native IoT post-boot service

`wor-iot-account` replaces the host logic of `src/lib/iot-account.py`. It
uses native OpenSSH only. The three existing Windows management payloads
are embedded unchanged at compile time and sent as PowerShell stdin; no
host Python/Bash wrapper or Windows installer is executed.

## Typed library boundary

- `factory_login_defaults() -> FactoryDefaults` returns the published maker
  defaults, not a fallback for missing configure credentials.
- `login_preferences(&Value)` and `account_preferences(&Value, metadata_only)`
  validate the legacy JSON protocol without DNS or network I/O and return
  typed `LoginPreferences` / `AccountPreferences`.
- `probe_device(host, automatic, &AccountOptions, &AtomicBool)` returns a
  typed `Pairing { address, key_type, key, fingerprint }`, without credentials.
- `ConfigureRequest::from_json(&Value)` adapts the existing helper request.
  `configure_account(&ConfigureRequest, &AccountOptions, &AtomicBool)` returns
  `ConfiguredAccount { state, username, sid, address, account_state,
  language_state, language }`, with the original camelCase JSON names.
- `AccountService { resolver: &dyn EndpointResolver, tools: &dyn NativeTools }`
  makes resolution and fixed native command execution testable.
- `interactive_request(...)` uses an injectable `Interaction`; the terminal
  implementation uses native masked `rpassword` input.

`Secret`, credential-bearing preferences/requests, `ManagementScript`, and
native tool requests deliberately do not implement Debug. `Secret::expose()`
is explicit. **Preferences JSON intentionally contains private credentials
for compatibility; never copy it into a summary, log, or Debug output.**
Password strings and encoded management scripts are zeroized on drop.

`AccountOptions` provides native companion, SSH and keyscan executable paths,
and `private_state_root`. The latter must be an existing, writable,
current-user directory with no group/world write permissions or symlink
ancestors; use the application's private state/cache directory. `new()`
defaults it to `HOME`, falling back to the current working directory if HOME
is absent. This also works from a read-only installed runtime/Finder cwd
without using a system temp directory.
Custom SSH executable paths should be absolute.

## Native askpass integration

The companion is selected by `AccountOptions.companion_executable`;
`AccountOptions::new()` uses `WOR_IOT_NATIVE_COMPANION`, then `current_exe()`.
Each operation creates a private directory containing:

```text
known_hosts          0600, literal wor-iot-paired identity
current-password     0600, only for the active SSH operation
wor-iot-askpass       symlink to the native companion
```

**Before Clap or ordinary runtime initialization**, the root CLI must check
its `argv[0]` basename. For `wor-iot-askpass`, call:

```text
wor_iot_account::run([OsString("askpass"), ...original SSH prompt arguments])
```

OpenSSH executes the symlink directly; there is no script trampoline.
Askpass only answers password prompts and refuses public, hard-linked,
symlinked, replaced, oversized or wrong-owner input. Its stdout is captured
by SSH, never sent to the normal application logger.

SSH uses a held private-directory descriptor as cwd, so the literal
`UserKnownHostsFile=known_hosts` cannot reinterpret spaces, percent tokens
or environment expressions in a parent pathname. The pinned known-host
contents and askpass symlink are rechecked. Host-key checking is strict;
global/user SSH config, public-key auth, multiplexing, local commands and
forwarding cannot override this operation. Credentials and encoded payloads
never enter command arguments.

## Preserved safety sequence

Automatic mode resolves **only `minwinpc`**, deduplicating address and IPv6
scope together; ambiguous automatic or any non-local result fails before
keyscan/credentials. Manual mode never scans either. The selected numeric
endpoint is revalidated, and explicit fingerprint consent is required.

Language setup precedes all account mutation. Its unchanged payload verifies
IoT Core, runs IoTSettings under DefaultAccount SID `-503` using an existing
interactive token (`LogonType=3`, `RunLevel=0`, no stored credentials), checks
installed language support, and cleans task/files. Cleanup/context/installed
tag failures block account changes. Inactive accepted settings remain
`pending-reboot`.

Account setup requires the local built-in administrator SID `-500`, applies
the requested name/password once, then makes a fresh pinned SSH login with
the desired credentials and verifies the same SID/name. Partial results,
reconnect failures and cleanup failures are unverified and never trigger an
automatic retry under the old credentials. Language-only results report the
account as `not-assessed`, not passed.

## Helper contract and tests

`run(&[OsString]) -> Result<i32, AccountError>` preserves `login-defaults`,
`preferences`, `probe`, `configure`, `askpass`, `--metadata-only`,
`--interactive`, `--language TAG`, and `--language-only`. JSON stays on
stdout; safe diagnostics stay on stderr. Status is 0 for a completed helper
operation, 1 for validation/SSH/I/O failure, 2 for syntax, and 130/143 for
handled SIGINT/SIGTERM. A `pending-reboot` JSON result is not verified success.

```text
cargo test --locked -p wor-iot-media -p wor-iot-account
cargo test --locked -p wor-iot-media --test mutations -- --ignored --nocapture
cargo clippy --locked -p wor-iot-media -p wor-iot-account --all-targets -- -D warnings
```

Native protocol tests reuse a Rust executable as fake SSH/keyscan and call the
real native askpass, including helper JSON/status, signals, hostile path
characters, fingerprint mismatch and cleanup. Unit/source-oracle tests cover
SID, credentials, automatic addressing, locale/task state, and malformed
responses. Disposable mutations disable consent, strict host-key checking,
same-SID verification and the DefaultAccount run-level rule and must fail.
No test contacts a real Pi; actual IoT Core/Windows task execution and hardware
qualification remain separate operator validation.
