# Contributing

Thanks for wanting to help. Blackout Secure directly maintains this source while
preserving Botspot's original authorship and the project's community connections.

## Table of contents

- [Where to send your change](#where-to-send-your-change)
- [Getting set up](#getting-set-up)
- [Running the tests](#running-the-tests)
- [House style](#house-style)
- [Traps that have bitten us](#traps-that-have-bitten-us)
- [Opening a pull request](#opening-a-pull-request)
- [Reporting a bug](#reporting-a-bug)

## Where to send your change

Open bug fixes, features and documentation changes at [Botspot/wor-flasher][issues].
Keep changes focused, preserve original attribution and explain user-visible behavior
in the pull request.

## Getting set up

```bash
git clone https://github.com/Botspot/wor-flasher
cd wor-flasher
```

You need `bash`, `shellcheck`, and Node.js 20 or newer for release packaging. For the
Linux integration suite on a non-Linux host, you also need Docker. Everything else the
scripts install for themselves.

## Running the tests

```bash
./tests/run-tests.sh          # static checks, plus Linux integration where available
./tests/run-tests.sh --gui    # walk through the GUI in DRY_RUN mode
./tests/run-linux-integration.sh   # force the Dockerised Linux suite
npm run check                 # shell syntax and an isolated release-package build
```

The Pi 3 post-install bootstrap helper is a small, self-contained Windows ARM64
program; users do not need Go to flash or install Windows. Its source is in
`src/pi3-boot-refresh/`, and the checked-in executable, source/binary hashes and
Go license are in `src/lib/pi3-boot-refresh/`.

When changing that helper, install Go 1.21 or newer (for example `brew install go`
on macOS), then run:

```bash
(cd src/pi3-boot-refresh && GO111MODULE=off GOTOOLCHAIN=local go test -count=1 ./...)
npm run build:pi3-boot-refresh
npm run pi3-boot-refresh:check
node --test tests/pi3-boot-refresh.test.mjs
```

The native core tests reuse the Python GPT/FAT fixtures and compare against the
desktop bootstrap helper, so they also need `python3`. The build cross-compiles
for Windows ARM64 v8.0 without CGO or external Go modules. Rebuild the executable
and its generated manifest/license together after source changes; normal release
packaging rejects a stale pair without requiring a Go compiler. Offline tests do
not execute Windows device-control calls or replace a real WoR-PE installation
test on a Pi.

The suite must be green and ShellCheck must be error-clean before a pull request can
be merged. CI runs the same commands on Ubuntu and macOS.

Two things to know about the harness:

- On macOS only the **Static checks** and **Shared engine functions** sections run.
  Everything else is gated behind `command -v losetup`. A test added elsewhere will
  not execute on macOS — validate it with `run-linux-integration.sh`.
- On a non-Linux host the run prints three summaries: the Docker container's nested
  run, the integration wrapper, then the host's own run. Every one must report
  `failed 0`. A failing host-side check is reported in its own summary, so do not
  read the container summary alone.
- Tests call the real functions out of `install-wor.sh` through the `run_in_engine`
  helper. Please do not restate product logic inside a test; a test that reimplements
  what it is checking can pass against code that no longer works.

**Mutation-test anything you add.** Break the behaviour on purpose, confirm your new
test fails, then put it back. A test that never fails is not a test.

## House style

The scripts follow Botspot's existing conventions, not a general shell style guide:

- `#comment` with no space after the hash.
- `if [ "$x" == y ];then` — no space before `;then`.
- Functions carry a one-line `#Input: ... Output: ...` comment on the same line as the
  opening brace.
- Comments explain _why_, not _what_. If the code already says it, do not repeat it.

Two structural rules matter more than any of that:

1. **`install-wor.sh` is the engine. `install-wor-gui.sh` is only windows.** Anything
   that decides _what gets written to the drive_ — validation, drive discovery,
   downloads, partitioning, the settings summary — belongs in `install-wor.sh` and is
   used by both front-ends. The GUI sources it. A function defined in both files is a
   bug, and there is a test that fails on it.
2. **The two front-ends must not diverge.** If you add a setting to the macOS Advanced
   Options window, add it to the Linux yad form and to `settings_summary` in the same
   commit.
3. **`install-wor-hook.sh` is the external process adapter, not an internal GUI layer.**
   Keep the built-in GUI on the shared engine functions it needs to construct its forms.
   Keep hook commands small, machine-readable and behaviorally covered by the test suite.

## Traps that have bitten us

Please read these before touching the relevant code. Each one cost real debugging time.

- **No apostrophes in the JXA heredocs.** The `<<'JXA'` heredocs live inside a
  `"$( ... )"` command substitution, so bash still tracks quote state through the body.
  One unpaired `'` — a comment saying "doesn't" — silently swallows the next several
  hundred lines and four function definitions. `bash -n` still passes. Write "does not".
- **`bash -n` is not enough.** Because of the above, the test suite runs on macOS in CI
  specifically to catch structural breakage in the JXA paths.
- **Never edit a script while it is running.** Bash reads scripts incrementally by byte
  offset. Editing `tests/run-tests.sh` mid-run corrupts execution and produces nonsense
  failures at valid lines.
- **`config-templates/` must stay tracked.** A missing template used to produce a blank
  `config.txt` and an unbootable Pi, with only a warning in a GUI that has no terminal.
- **A subshell cannot `wait` on a sibling.** Let the background job record its own exit
  status; do not wrap it in `( wait "$pid"; ... ) &`.

## Opening a pull request

- One logical change per pull request.
- Say what you tested it on. "Flashed a Pi 4 from Debian Bookworm to a 64 GB SanDisk"
  is worth more than any amount of review.
- If you changed behaviour, update the README and the version history at the top of
  `install-wor.sh` in the same commit.
- For a release, update `WOR_FLASHER_VERSION` in `src/lib/metadata.sh` and both version
  fields in `src/macos-app/Contents/Info.plist`; the test suite checks all release metadata.
- Be honest about what you did not test. Nobody has every Pi model.

## Reporting a bug

Use the [issue templates][issues]. Include:

- Host OS and version, and Raspberry Pi model.
- The exact command or GUI choices you used.
- The timestamped log shown in the failure dialog, or `$DL_DIR/last-run.log` as the latest-run
  shortcut. Set `WOR_LOG_FILE` to put it somewhere else.
- What you expected, and what happened instead.

For real-time help, the [Botspot Software Discord][botspot-discord] is the best place
for WoR-Flasher itself, and the [WoR project Discord][wor-discord] is the place for
questions about Windows on Raspberry as an operating system.

[issues]: https://github.com/Botspot/wor-flasher/issues/new/choose
[botspot-discord]: https://discord.gg/RXSTvaUvuu
[wor-discord]: https://discord.gg/jQCpfVK
