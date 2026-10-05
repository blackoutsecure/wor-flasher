import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

const root = fileURLToPath(new URL("../", import.meta.url));
const engine = readFileSync(join(root, "install-wor.sh"), "utf8");
const preparationStart = engine.indexOf("{ #Download Windows ESD");
const preparationEnd = engine.indexOf("fi #end of the preparation half", preparationStart);
assert.ok(preparationStart >= 0 && preparationEnd > preparationStart);
const preparation = engine.slice(preparationStart, preparationEnd);

function run(body, version = 10) {
  const directory = mkdtempSync(join(tmpdir(), "wor-windows-progress-"));
  try {
    mkdirSync(join(directory, "downloads"));
    const result = spawnSync("bash", ["-c", `
      source "$DIRECTORY/install-wor.sh" source >/dev/null || exit 90
      BID="$TEST_BUILD" WINDOWS_VER="$TEST_STALE_LABEL" WIN_LANG=en-us
      RPI_MODEL=3 CAN_INSTALL_ON_SAME_DRIVE=1 DEVICE=/dev/mock-only
      DL_DIR="$TEST_DIRECTORY/downloads" SOURCE_FILE='' HOST_OS=Darwin
      WOR_GUI_PROGRESS_FILE="$TEST_DIRECTORY/progress"
      STEP_NUM=3 STEP_TOTAL=8
      cd "$DL_DIR" || exit 91
      ${body}
    `], {
      cwd: directory, encoding: "utf8", timeout: 15000,
      env: {
        ...process.env, NO_UPDATE: "1", DIRECTORY: root, WOR_CACHE_DIR: join(directory, "cache"),
        TEST_DIRECTORY: directory, TEST_BUILD: version === 10 ? "19045.3803" : "22631.2861",
        TEST_STALE_LABEL: version === 10 ? "Windows 11" : "Windows 10",
      },
    });
    const read = (name) => existsSync(join(directory, name)) ? readFileSync(join(directory, name), "utf8") : "";
    return { ...result, progress: read("progress"), notification: read("notification") };
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

const preparationMocks = `
  using_esd=true
  cache_downloader() {
    printf '%s\\n' '<File><LanguageCode>en-us</LanguageCode><FilePath>https://example.invalid/windows.esd</FilePath><Size>64</Size><Sha1>fixture-digest</Sha1></File>'
  }
  require_free_space() { :; }
  wget() {
    [ "$1" == https://example.invalid/windows.esd ] && [ "$2" == -O ] || return 99
    printf 'ESD fixture\\n' > "$3"
  }
  sha1_file_with_progress() { printf 'fixture-digest'; }
  wimextract() {
    local argument
    for argument;do
      case "$argument" in --dest-dir=*) mkdir -p "\${argument#--dest-dir=}/boot" "\${argument#--dest-dir=}/efi" ;; esac
    done
    printf '100%%\\n' >&2
  }
  wimexport() { printf 'WIM fixture\\n' >> "$3"; printf '100%%\\n' >&2; }
  wimdelete() { printf '100%%\\n' >&2; }
  copy_local_file_with_progress() { emit_gui_task_progress 100 "$1"; cp "$2" "$3"; }
  darwin_mount_iso() {
    ISO_MOUNTPOINT="$PWD/isomount" ISO_DEVICE=/dev/mock-iso
    mkdir -p "$ISO_MOUNTPOINT/boot" "$ISO_MOUNTPOINT/efi" "$ISO_MOUNTPOINT/sources"
    printf 'PE fixture\\n' > "$ISO_MOUNTPOINT/sources/boot.wim"
    printf 'Windows fixture\\n' > "$ISO_MOUNTPOINT/sources/install.wim"
  }
  register_device_cleanup() { :; }
  hdiutil() { [ "$*" == 'detach /dev/mock-iso' ]; }
`;

describe("Windows-specific image preparation progress", () => {
  for (const version of [10, 11]) {
    for (const cached of ["winfiles", "winfiles_from_iso"]) {
      it(`identifies Windows ${version} when reusing ${cached}`, () => {
        const result = run(`
          mkdir -p "${cached}_\${BID}_\${WIN_LANG}"
          touch "${cached}_\${BID}_\${WIN_LANG}/alldone"
          wget() { printf 'Unexpected network access\\n' >&2; return 99; }
          ${preparation}
        `, version);
        assert.equal(result.status, 0, result.stderr);
        assert.match(result.progress, new RegExp(`^STEP\\t4\\t8\\tPreparing the Windows ${version} image$`, "m"));
        assert.match(result.progress, new RegExp(`^STATUS\\tReusing .*Windows ${version}.*already extracted`, "m"));
        assert.doesNotMatch(result.progress, new RegExp(`Windows ${version === 10 ? 11 : 10}`));
      });
    }

    it(`names Windows ${version} throughout fresh ESD download and extraction`, () => {
      const result = run(`${preparationMocks}\n${preparation}`, version);
      assert.equal(result.status, 0, result.stdout + result.stderr);
      for (const text of [
        `Preparing the Windows ${version} image`,
        `Downloading Windows ${version} ESD image`,
        `Verifying downloaded Windows ${version} image`,
        `Extracting Windows ${version} files from image.esd`,
        `Exporting Windows ${version} image`,
      ]) assert.ok(result.progress.includes(text), `Missing progress: ${text}\n${result.progress}`);
      assert.match(result.stdout + result.stderr, new RegExp(`Windows ${version} image download verified`));
    });

    it(`names Windows ${version} when importing an ISO`, () => {
      const result = run(`
        ${preparationMocks}
        SOURCE_FILE="$TEST_DIRECTORY/import.iso"
        ${preparation}
      `, version);
      assert.equal(result.status, 0, result.stdout + result.stderr);
      for (const text of [
        `Preparing the Windows ${version} image`,
        `Using the selected Windows ${version} ISO`,
        `Mounting Windows ${version} ISO`,
        `Copying Windows ${version} files from ISO`,
        `Windows ${version} PE image (boot.wim)`,
        `Unmounting Windows ${version} ISO`,
      ]) assert.ok(result.progress.includes(text), `Missing progress: ${text}\n${result.progress}`);
    });
  }

  it("changes the next message when the selected build changes in the same process", () => {
    const result = run(`
      phase "Preparing the $(windows_version_label) image"
      BID=22631.2861
      phase "Preparing the $(windows_version_label) image"
    `);
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.progress, /STEP\t4\t8\tPreparing the Windows 10 image/);
    assert.match(result.progress, /STEP\t5\t8\tPreparing the Windows 11 image/);
  });
});

describe("Version-specific image subtasks and notifications", () => {
  for (const version of [10, 11]) {
    it(`identifies Windows ${version} in tool subprogress without changing explicit labels`, () => {
      const result = run(`
        progress_task_label /opt/bin/wimextract source.esd 1 boot efi
        progress_task_label wimexport source.esd 2 boot.wim
        progress_task_label wimdelete image.esd 1
        progress_task_label sudo wimverify /tmp/boot.wim
        progress_task_label pv -f -N "chosen-label" /tmp/file
        BID=''
        progress_task_label wimexport source.esd 2 boot.wim
      `, version);
      assert.equal(result.status, 0, result.stderr);
      assert.deepEqual(result.stdout.trim().split("\n"), [
        `Extracting Windows ${version} files`,
        `Exporting Windows ${version} image`,
        `Preparing Windows ${version} installation image`,
        `Verifying Windows ${version} image (boot.wim)`,
        "chosen-label", "wimexport",
      ]);
    });

    for (const platform of ["macos", "linux"]) {
      it(`names Windows ${version} in ${platform} result notifications`, () => {
        const result = run(`
          SHOW_NOTIFICATION=1
          wor_host_platform() { printf '%s' ${platform}; }
          wor_osascript() { cat >/dev/null; printf '%s\\n' "$3" >> "$TEST_DIRECTORY/notification"; }
          notify-send() { printf '%s\\n' "$2" >> "$TEST_DIRECTORY/notification"; }
          wor_show_result_notification success "$(windows_version_label)"
          wait
          wor_show_result_notification failure "$(windows_version_label)"
          wait
        `, version);
        assert.equal(result.status, 0, result.stderr);
        assert.deepEqual(result.notification.trim().split("\n"), [
          `Finished preparing Windows ${version} media on /dev/mock-only. It is ready to boot on your Raspberry Pi.`,
          `Preparing Windows ${version} media stopped before it finished. Open WoR-Flasher for details.`,
        ]);
      });
    }
  }
});

describe("Windows-specific written-image verification", () => {
  const verificationMocks = `
    STEP_NUM=6 STEP_TOTAL=8
    OOBE_NETWORK_BYPASS=0 WINDOWS_ACCOUNT_SETUP=0 WINDOWS_LOCALE_SETUP=0 PI4_AUTO_DISABLE_3GB=0
    sync() { :; }
    darwin_plist_json() { printf '%s\\n' '{"AllDisksAndPartitions":[{"Partitions":[{},{}]}]}'; }
    darwin_device_value() {
      case "$2" in
        .Content) [ "$1" == /dev/mock-boot ] && printf EFI || printf 'Microsoft Basic Data' ;;
        .FilesystemType) [ "$1" == /dev/mock-boot ] && printf msdos || printf exfat ;;
        .VolumeName) [ "$1" == /dev/mock-boot ] && printf WOR_BOOT || printf WOR_INSTALL ;;
        '.TotalSize // .Size') printf 64000000000 ;;
        .PartitionMapPartitionOffset) printf 1610612736 ;;
        .Size) printf 18874368000 ;;
        *) printf 'Unexpected metadata lookup\\n' >&2; return 99 ;;
      esac
    }
    mounted_test() { :; }
    wimverify() {
      printf '100%%\\n' >&2
      [ "\${FAIL_INSTALL_WIM:-0}" != 1 ] || [ "\${1##*/}" != install.wim ]
    }
    sha256_file_with_progress() { printf 'fixture-digest'; }
  `;

  for (const version of [10, 11]) {
    it(`identifies Windows ${version} without changing verification progress or success status`, () => {
      const result = run(`
        ${verificationMocks}
        verify_written_image /dev/mock-only /dev/mock-boot /dev/mock-install "$TEST_DIRECTORY/boot" "$TEST_DIRECTORY/install" "$TEST_DIRECTORY/source.wim"
      `, version);
      assert.equal(result.status, 0, result.stderr);
      assert.match(result.progress, new RegExp(`^STEP\\t7\\t8\\tVerifying the written Windows ${version} image$`, "m"));
      for (const [percent, text] of [
        [45, `Verifying Windows ${version} PE image integrity (boot.wim)`],
        [60, `Verifying Windows ${version} installation image integrity (install.wim)`],
        [75, `Hashing source Windows ${version} installation image`],
        [88, `Hashing written Windows ${version} installation image`],
        [100, `Windows ${version} written image verified`],
      ]) assert.ok(result.progress.includes(`TASK\t${percent}\t${text}\n`), `Missing task: ${text}`);
      assert.match(result.stdout + result.stderr, new RegExp(`Written image verified successfully \\(Windows ${version}\\)`));
    });

    it(`keeps a Windows ${version} verification failure explicit rather than reporting success`, () => {
      const result = run(`
        ${verificationMocks}
        FAIL_INSTALL_WIM=1
        verify_written_image /dev/mock-only /dev/mock-boot /dev/mock-install "$TEST_DIRECTORY/boot" "$TEST_DIRECTORY/install" "$TEST_DIRECTORY/source.wim"
      `, version);
      assert.equal(result.status, 1);
      assert.match(result.stderr, new RegExp(`Windows ${version} install.wim is invalid or corrupted`));
      assert.doesNotMatch(result.stdout + result.stderr, /Written image verified successfully/);
      assert.doesNotMatch(result.progress, /^TASK\t100\tWindows .* written image verified/m);
    });
  }
});
