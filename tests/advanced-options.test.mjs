import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";
import vm from "node:vm";

const root = fileURLToPath(new URL("../", import.meta.url));
const gui = readFileSync(join(root, "install-wor-gui.sh"), "utf8");
const engine = readFileSync(join(root, "install-wor.sh"), "utf8");
const macosStart = gui.indexOf("macos_advanced_options() {");
const macosEnd = gui.indexOf("\nmacos_choose_target() {", macosStart);
assert.ok(macosStart >= 0 && macosEnd > macosStart, "Missing macOS Advanced Options");
const macos = gui.slice(macosStart, macosEnd);
const sharedGui = ["prepare_release_choices", "release_choice_tag", "save_advanced_preferences", "restore_advanced_preferences"].map((name) => {
  const match = gui.match(new RegExp(`^${name}\\(\\) \\{[\\s\\S]*?^\\}`, "m"));
  assert.ok(match, `Missing ${name}`);
  return match[0];
}).join("\n");
const linuxStart = gui.indexOf("    refresh_prompt=() #this variable is populated if the Advanced Options window is repeated");
const linuxEnd = gui.indexOf("    done #end of repeating the advanced options window", linuxStart);
assert.ok(linuxStart >= 0 && linuxEnd > linuxStart, "Missing Linux Advanced Options");
const linux = gui.slice(linuxStart, gui.indexOf("\n", linuxEnd));
const jxa = macos.match(/advanced_jxa="\$\(wor_jxa_window_lib; cat <<'JXA'\n([\s\S]*?)\nJXA/);
assert.ok(jxa, "Missing native Advanced Options renderer");
const firmwareStart = engine.indexOf('phase "Preparing Pi${RPI_MODEL} UEFI firmware"');
const firmwareEnd = engine.indexOf("{ #Download Windows ESD", firmwareStart);
assert.ok(firmwareStart >= 0 && firmwareEnd > firmwareStart);
const firmware = engine.slice(firmwareStart, firmwareEnd);

const fixtureSetup = `
source "$TEST_ROOT/install-wor.sh" source >/dev/null || exit 90
source "$TEST_DIR/shared-gui.sh"
RPI_MODEL="$TEST_MODEL" BID="$TEST_BUILD" CAN_INSTALL_ON_SAME_DRIVE="$TEST_MODE"
DL_DIR="$TEST_DIR/downloads" SOURCE_FILE='' WIN_LANG=en-us WINDOWS_LOCALE=en-US
DEVICE=/dev/mock-only CONFIG_TXT=fixture-config APPLY_CUSTOM_CONFIG_TXT=1
WINDOWS_ACCOUNT_SETUP=0 WINDOWS_ACCOUNT_USERNAME=fixture-user WINDOWS_ACCOUNT_PASSWORD=fixture-password
WINDOWS_LOCALE_SETUP=1 OOBE_NETWORK_BYPASS="$TEST_OOBE_OVERRIDE" PI4_AUTO_DISABLE_3GB=1 DRIVERS_USE_LATEST=1
UEFI_USE_LATEST="$TEST_UEFI_OVERRIDE" SKIP_IMAGE_VERIFICATION=0 DRY_RUN=0 USE_CACHE=1 PLAY_SOUND=1 SHOW_NOTIFICATION=1
WOR_ICON_PATH=fixture-icon
WOR_YAD_SCREEN_WIDTH=1920 WOR_YAD_SCREEN_HEIGHT=1080
list_langs_preferred() { printf 'en-us:English\\nfr-fr:French\\n'; }
list_windows_locale_options() { printf 'en-US\\tEnglish\\nfr-FR\\tFrench\\n'; }
wor_sound_options() { printf 'Glass\\tGlass\\n'; }
wor_completion_sound() { printf 'Glass'; }
is_known_win_lang() { [ "$1" == en-us ] || [ "$1" == fr-fr ]; }
describe_device() { printf '%s' "$1"; }
list_release_versions() {
  if [ "\${2:-all}" == latest ];then
    case "$1" in uefi) uefi_pinned_version ;; drivers) printf '%s\\n' "$DRIVER_VER" ;; esac
    return
  fi
  case "$1" in
    uefi) printf '%s\\nv1.20\\n' "$(uefi_pinned_version)" ;;
    drivers) printf '%s\\nv0.16\\n' "$DRIVER_VER" ;;
  esac
}
if [ "$TEST_ISO" == source ];then SOURCE_FILE="$TEST_DIR/selected.iso";fi
if [ "$TEST_ISO" == cached ];then
  mkdir -p "$DL_DIR/winfiles_from_iso_\${BID}_\${WIN_LANG}"
  touch "$DL_DIR/winfiles_from_iso_\${BID}_\${WIN_LANG}/alldone"
fi
`;

function run(body, {
  version = 11, model = 4, mode = 1, iso = "", edit = false, cancel = false,
  uefiOverride = "0", uefiSelection = "", oobeOverride = "1", driverSelection = "",
} = {}) {
  const directory = mkdtempSync(join(tmpdir(), "wor-advanced-"));
  try {
    mkdirSync(join(directory, "downloads"));
    writeFileSync(join(directory, "macos.sh"), macos);
    writeFileSync(join(directory, "shared-gui.sh"), sharedGui);
    const result = spawnSync("bash", ["-c", `${fixtureSetup}\n${body}`], {
      cwd: directory, encoding: "utf8", timeout: 20000,
      env: {
        ...process.env, NO_UPDATE: "1", DIRECTORY: root, WOR_CACHE_DIR: join(directory, "cache"),
        TEST_ROOT: root, TEST_DIR: directory, TEST_MODEL: String(model), TEST_MODE: String(mode),
        TEST_BUILD: version === 10 ? "19045.3803" : "22631.2861", TEST_ISO: iso, WOR_RUN_ID: "advanced-fixture",
        TEST_EDIT: edit ? "1" : "0", TEST_CANCEL: cancel ? "1" : "0",
        TEST_UEFI_OVERRIDE: uefiOverride, TEST_UEFI_SELECTION: uefiSelection,
        TEST_OOBE_OVERRIDE: oobeOverride,
        TEST_DRIVER_SELECTION: driverSelection,
      },
    });
    const read = (name) => existsSync(join(directory, name)) ? readFileSync(join(directory, name), "utf8") : "";
    return {
      ...result, args: read("args").split("\0").slice(0, -1), state: read("state"),
      summary: read("summary").replaceAll(directory, "[fixture]"),
      oobePreference: read("oobe-preference").trim(), answer: read("answer.xml"),
      overlayScrolling: read("overlay-scrolling").trim(),
      releaseState: read("release-state").trim(),
      releaseFlags: read("release-flags").trim(),
    };
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

const macosProbe = `
source "$TEST_DIR/macos.sh"
wor_osascript() {
  cat >/dev/null
  printf '%s\\0' "$@" > "$TEST_DIR/args"
  [ "$TEST_CANCEL" != 1 ] || { printf 'CANCEL\\n'; return 0; }
  if [ "$TEST_EDIT" == 1 ];then
    printf 'OK\\n0\\n0\\n1\\n0\\n1\\n1\\n0\\n1\\n0\\n2\\nfixture-user\\nfixture-password\\nfr-FR\\nfr-fr\\n0\\nGlass\\n0\\n%s\\n%s\\n---CONFIG_TXT---\\nedited-config\\n' "$uefi_dropdown_default" "$driver_dropdown_default"
  else
    printf 'OK\\n%s\\n1\\n%s\\n%s\\n0\\n0\\n0\\n1\\n1\\n1\\nfixture-user\\nfixture-password\\nen-US\\nen-us\\n1\\nGlass\\n1\\n%s\\n%s\\n---CONFIG_TXT---\\nfixture-config\\n' "$OOBE_NETWORK_BYPASS" "\${TEST_UEFI_SELECTION:-$(uefi_use_latest)}" "\${TEST_DRIVER_SELECTION:-$DRIVERS_USE_LATEST}" "$uefi_dropdown_default" "$driver_dropdown_default"
  fi
}
macos_advanced_options
printf '%s|%s|%s|%s|%s|%s|%s|%s|%s|%s\\n' "$PI4_AUTO_DISABLE_3GB" "$DRIVERS_USE_LATEST" "$UEFI_USE_LATEST" "$SKIP_IMAGE_VERIFICATION" "$DRY_RUN" "$WIN_LANG" "$USE_CACHE" "$WINDOWS_LOCALE" "$APPLY_CUSTOM_CONFIG_TXT" "$CONFIG_TXT" > "$TEST_DIR/state"
settings_summary > "$TEST_DIR/summary"
printf '%s\\n' "$OOBE_NETWORK_BYPASS" > "$TEST_DIR/oobe-preference"
printf '%s\\n' "\${WOR_SELECTED_RELEASES:-}" > "$TEST_DIR/release-flags"
`;

const linuxProbe = `
yad() {
  printf '%s\\0' "$@" > "$TEST_DIR/args"
  printf '%s\\n' "\${GTK_OVERLAY_SCROLLING:-unset}" > "$TEST_DIR/overlay-scrolling"
  [ "$TEST_CANCEL" != 1 ] || return 1
  local index label value
  for ((index=0; index<\${#fields[@]}; index+=2));do
    label="\${fields[$index]}" value="\${fields[$((index+1))]}"
    case "$label" in
      *:CHK)
        if [ "$TEST_EDIT" == 1 ];then
          case "$label" in
            *'latest UEFI'*|*'Skip verifying'*|*'dry run'*|*'regional settings'*) value=TRUE ;;
            *) value=FALSE ;;
          esac
        fi ;;
      *'Downloaded files:CB') [ "$TEST_EDIT" != 1 ] || value='Trust the cache without checking it' ;;
      *'Choose Windows language:CB') [ "$TEST_EDIT" != 1 ] || value='fr-fr: French' ;;
      *'Windows locale:CB') [ "$TEST_EDIT" != 1 ] || value='fr-FR: French' ;;
    esac
    if [[ "$label" == *'latest UEFI'* ]] && [ -n "$TEST_UEFI_SELECTION" ];then
      [ "$TEST_UEFI_SELECTION" == 1 ] && value=TRUE || value=FALSE
    fi
    printf '%s\\n' "\${value%%!*}"
  done
}
${linux}
printf '%s|%s|%s|%s|%s|%s|%s|%s|%s\\n' "$PI4_AUTO_DISABLE_3GB" "$DRIVERS_USE_LATEST" "$UEFI_USE_LATEST" "$SKIP_IMAGE_VERIFICATION" "$DRY_RUN" "$WIN_LANG" "$USE_CACHE" "$WINDOWS_LOCALE" "$APPLY_CUSTOM_CONFIG_TXT" > "$TEST_DIR/state"
settings_summary > "$TEST_DIR/summary"
printf '%s\\n' "$OOBE_NETWORK_BYPASS" > "$TEST_DIR/oobe-preference"
printf '%s\\n' "\${WOR_SELECTED_RELEASES:-}" > "$TEST_DIR/release-flags"
`;

function renderedMacCheckboxes(spec, setupTitle) {
  const headers = [], labels = [], annotations = [], controls = [];
  const rowsStart = jxa[1].indexOf("const rows = checkboxSpec.split");
  const rowsEnd = jxa[1].indexOf("\n\nconst langOptions", rowsStart);
  assert.ok(rowsStart >= 0 && rowsEnd > rowsStart);
  const start = jxa[1].indexOf("for (let i = 0; i < rows.length; i++) {");
  const end = jxa[1].indexOf("addSectionHeader('Downloads')", start);
  assert.ok(start >= 0 && end > start);
  const context = vm.createContext({
    checkboxSpec: spec, setupTitle, addSectionHeader: (value) => headers.push(value),
    checkboxes: [], width: 640, y: 1000, rowHeight: 26, controller: {},
    addVersionSelector: () => {},
    worAnnotateCheckbox: (checkbox, label, note, color) => annotations.push({ label, note, color }),
    content: { addSubview(checkbox) { labels.push(checkbox.label); controls.push(checkbox); } },
    $: {
      NSButton: { checkboxWithTitleTargetAction: (label) => ({ label }) },
      NSMakeRect: () => ({}), NSColor: { systemRedColor: "red", systemGreenColor: "green" },
      NSViewWidthSizable: 1, NSViewMinYMargin: 2,
    },
  });
  vm.runInContext(jxa[1].slice(rowsStart, rowsEnd), context);
  vm.runInContext(jxa[1].slice(start, end), context);
  return { labels, headers, annotations, controls };
}

describe("Advanced Options Windows/model/mode matrix", () => {
  for (const version of [10, 11]) {
    for (const model of [3, 4, 5]) {
      for (const mode of [0, 1]) {
        it(`shows applicable options for Windows ${version}, Pi ${model}, mode ${mode}`, () => {
          const options = { version, model, mode };
          const mac = run(macosProbe, options);
          const lin = run(linuxProbe, options);
          assert.equal(mac.status, 0, mac.stderr);
          assert.equal(lin.status, 0, lin.stderr);
          const scope = mode === 0 ? "destination drive" : "this drive";
          assert.match(mac.args[24], new RegExp(`Windows ${version} setup on (the )?${scope}`));
          assert.match(mac.args[23], mode === 0 ? /Creating recovery media.*\n.*another drive/ : /installation on this drive/);
          const { labels, headers } = renderedMacCheckboxes(mac.args[3], mac.args[24]);
          const linuxLabels = lin.args.filter((value) => value.startsWith("--field="));
          const filesStatusIndex = lin.args.indexOf("--field=Windows files:RO");
          assert.ok(filesStatusIndex >= 0);
          assert.equal(lin.args[filesStatusIndex + 1], `Will download and extract the Windows ${version} image`);
          for (const group of [labels, linuxLabels]) {
            assert.equal(group.some((label) => label.includes("Pi 4 3 GB")), model === 4);
            assert.equal(group.some((label) => label.includes("latest Windows ARM64 drivers")), model !== 5);
            assert.equal(group.some((label) => label.includes("without a network connection")), version === 11);
            assert.equal(group.some((label) => label.includes("Skip Windows 10 network")), false);
            assert.equal(group.some((label) => /not applicable/i.test(label)), false);
            assert.equal(group.some((label) => label.includes("Skip verifying the prepared recovery media")), mode === 0);
          }
          assert.ok(headers.includes("Firmware and drivers") && headers.includes("Validation"));
          assert.equal(headers.includes(mac.args[24]), version === 11 || model === 4);
          assert.equal(linuxLabels.some((label) => label === `--field=${mac.args[24]}:LBL`), version === 11 || model === 4);
          assert.ok(linuxLabels.some((label) => label.includes("Create an optional local Windows")));
          assert.ok(linuxLabels.some((label) => label.includes("Configure Windows keyboard")));
          assert.match(mac.args[8], mode === 0 ? /recovery media, not the Windows drive/ : /boot partition/);
          assert.equal(mac.summary, lin.summary);
          assert.equal(mac.summary.includes("Offline OOBE\t"), version === 11);
          assert.doesNotMatch(mac.summary, /fixture-password/);
          if (model === 5) assert.match(mac.summary, /No separate driver package for Pi 5/);
          if (mode === 0) {
            assert.match(mac.summary, new RegExp(`Windows setup scope\\tWindows ${version} setup on the destination drive`));
            assert.match(mac.summary, /Custom config\.txt\tApplied \(recovery media only\)/);
          }
        });
      }
    }
  }
});

describe("ARM64 driver recommendations", () => {
  for (const model of [3, 4, 5]) {
    it(`recommends latest drivers only where applicable on Pi ${model}`, () => {
      const mac = run(macosProbe, { model });
      const lin = run(linuxProbe, { model });
      assert.equal(mac.status, 0, mac.stderr);
      assert.equal(lin.status, 0, lin.stderr);
      const rendered = renderedMacCheckboxes(mac.args[3], mac.args[24]);
      const control = rendered.controls.find((item) => item.label.includes("latest Windows ARM64 drivers"));
      const field = lin.args.findIndex((item) => item.startsWith("--field=") && item.includes("latest Windows ARM64 drivers"));
      assert.equal(mac.args[3].split("\n")[3].split("\t")[4], model === 5 ? "0" : "1");
      if (model === 5) {
        assert.equal(control, undefined);
        assert.equal(field, -1);
      } else {
        assert.ok(control);
        assert.equal(control.state, 1);
        assert.deepEqual(rendered.annotations.find((item) => item.label === control.label), {
          label: control.label, note: "Recommended", color: "green",
        });
        assert.ok(field >= 0);
        assert.match(lin.args[field], /\(recommended\)/);
        assert.equal(lin.args[field].split(":").length, 2, "Only the YAD field-type separator may contain a colon");
        assert.equal(lin.args[field + 1], "TRUE");
      }
    });
  }

  for (const model of [3, 4]) {
    it(`preserves explicit pinned drivers on Pi ${model}`, () => {
      for (const probe of [macosProbe, linuxProbe]) {
        const result = run(`DRIVERS_USE_LATEST=0\n${probe}`, { model });
        assert.equal(result.status, 0, result.stderr);
        assert.equal(result.state.split("|")[1], "0");
        assert.match(result.summary, /Windows ARM64 drivers\tPinned \(v0\.17\)/);
      }
    });
  }
});

describe("Advanced Options fit and scrolling", () => {
  const start = jxa[1].indexOf("function layoutAdvancedOptions()");
  const end = jxa[1].indexOf("const okButton =", start);
  assert.ok(start >= 0 && end > start, "Missing content-measured options layout");

  for (const measuredHeight of [30, 45.5]) {
    it(`fits the intro to its ${measuredHeight}-point wrapped text instead of reserving blank lines`, () => {
      const introStart = jxa[1].indexOf("const contextLabel =");
      const introEnd = jxa[1].indexOf("function addSectionHeader", introStart);
      assert.ok(introStart >= 0 && introEnd > introStart);
      let measuredWidth;
      const label = {
        cell: { cellSizeForBounds(bounds) {
          measuredWidth = bounds.size.width;
          return { height: measuredHeight };
        } },
      };
      const context = vm.createContext({
        contextText: "Windows setup scope", width: 640, y: 0, content: { addSubview: () => {} },
        $: {
          NSTextField: { wrappingLabelWithString: () => label },
          NSFont: { systemFontOfSize: () => ({}) },
          NSScroller: { scrollerWidthForControlSizeScrollerStyle: () => 17 },
          NSControlSizeRegular: 0, NSScrollerStyleLegacy: 0,
          NSMakeRect: (x, y, width, height) => ({ origin: { x, y }, size: { width, height } }),
          NSViewWidthSizable: 1, NSViewMinYMargin: 2,
        },
      });
      vm.runInContext(jxa[1].slice(introStart, introEnd), context);
      assert.equal(measuredWidth, 583);
      assert.equal(label.frame.size.height, Math.ceil(measuredHeight));
      assert.equal(label.frame.origin.y, -Math.ceil(measuredHeight));
      assert.equal(context.y, -(Math.ceil(measuredHeight) + 16));
    });
  }

  function layout(screenHeight, span = 600, chromeHeight = 28) {
    const views = [0, 1].map((index) => ({
      frame: {
        origin: { x: 20, y: index === 0 ? -40 : -span },
        size: { width: 600, height: index === 0 ? 40 : 28 },
      },
      setFrameOrigin(point) { this.frame.origin = point; },
    }));
    const rootViews = [];
    const content = {
      frame: { origin: { x: 0, y: 0 }, size: { width: 640, height: 0 } },
      subviews: { count: views.length, objectAtIndex: (index) => views[index] },
      autoresizesSubviews: false,
      setFrameSize(size) { this.frame.size = size; },
      setFrameOrigin(point) { this.frame.origin = point; },
      scrollPoint(point) { this.scrollPosition = point; },
    };
    const window = {
      frame: { size: { width: 640, height: chromeHeight + 1 } },
      contentView: { frame: { size: { width: 640, height: 1 } }, addSubview: (view) => rootViews.push(view) },
      setContentSize(size) {
        this.contentView.frame.size = size;
        this.frame.size.height = size.height + chromeHeight;
      },
    };
    let createdScroller = null;
    const context = vm.createContext({
      content, window, screenFrame: { size: { height: screenHeight } }, width: 640,
      baselineFrames: views.map((view) => ({ view, ...view.frame.origin, ...view.frame.size })),
      versionRows: [], checkboxes: [], bodyHeight: 0, scrollHeight: 0, needsScrolling: false, scrollView: null,
      $: {
        NSMakeSize: (width, height) => ({ width, height }),
        NSMakePoint: (x, y) => ({ x, y }),
        NSMakeRect: (x, y, width, height) => ({ origin: { x, y }, size: { width, height } }),
        NSScrollerStyleLegacy: 0,
        NSScrollView: { alloc: { initWithFrame(frame) {
          createdScroller = { frame, contentSize: { width: frame.size.width - 16, height: frame.size.height } };
          return createdScroller;
        } } },
      },
    });
    vm.runInContext(jxa[1].slice(start, end), context);
    return { content, views, window, rootViews, scroller: createdScroller };
  }

  for (const height of [900, 734]) {
    it(`does not create a scroll area when controls fit a ${height}-point screen`, () => {
      const result = layout(height);
      assert.equal(result.scroller, null);
      assert.equal(result.content.frame.size.height, 628);
      assert.equal(result.content.frame.origin.y, 70);
      assert.equal(result.window.frame.size.height, 734);
      const topControl = Math.max(...result.views.map((view) => view.frame.origin.y + view.frame.size.height));
      assert.equal(result.content.frame.size.height - topControl, 8);
      assert.equal(result.window.contentView.frame.size.height - result.content.frame.origin.y - result.content.frame.size.height, 8);
      assert.equal(result.rootViews[0], result.content);
      assert.equal(result.content.scrollPosition, undefined);
    });
  }

  for (const height of [733, 600]) {
    it(`shows a visible scrollbar and starts at the top when controls overflow a ${height}-point screen`, () => {
      const result = layout(height);
      assert.ok(result.scroller);
      assert.equal(result.scroller.hasVerticalScroller, true);
      assert.equal(result.scroller.hasHorizontalScroller, false);
      assert.equal(result.scroller.autohidesScrollers, false);
      assert.equal(result.scroller.scrollerStyle, 0);
      assert.equal(result.scroller.documentView, result.content);
      assert.equal(result.content.frame.size.width, result.scroller.contentSize.width);
      assert.equal(result.content.scrollPosition.y, 734 - height);
      assert.equal(result.window.frame.size.height, height);
      assert.equal(result.rootViews[0], result.scroller);
    });
  }

  it("measures actual control bounds with padding instead of a fixed row-count estimate", () => {
    const result = layout(900, 473.5);
    assert.equal(result.content.frame.size.height, 502);
    for (const view of result.views) {
      assert.ok(view.frame.origin.y >= 20);
      assert.ok(view.frame.origin.y + view.frame.size.height <= 494);
    }
    assert.equal(result.window.frame.size.height, 608);
    assert.match(jxa[1], /window\.contentView\.addSubview\(okButton\)/);
    assert.match(jxa[1], /window\.contentView\.addSubview\(cancelButton\)/);
  });

  it("lets Linux use available screen height and GTK automatic scrollbar visibility", () => {
    const result = run(linuxProbe);
    assert.equal(result.status, 0, result.stderr);
    assert.ok(result.args.includes("--height=1020"));
    assert.ok(result.args.includes("--vscroll-policy=auto"));
    assert.equal(result.overlayScrolling, "0");
  });
});

describe("Tagged release selectors", () => {
  for (const model of [3, 4, 5]) {
    it(`loads firmware choices and only applicable driver choices for Pi ${model}`, () => {
      const mac = run(macosProbe, { model });
      assert.equal(mac.status, 0, mac.stderr);
      assert.match(mac.args[25], /v1\.20/);
      assert.equal(mac.args[27].includes("v0.16"), model !== 5);
      const lin = run(linuxProbe, { model });
      assert.equal(lin.status, 0, lin.stderr);
      assert.ok(lin.args.some((arg) => arg === "--field=UEFI version:CB"));
      assert.equal(lin.args.some((arg) => arg === "--field=Driver version:CB"), false);
    });
  }

  it("hides both Linux dropdowns while their latest choices are enabled", () => {
    const result = run(linuxProbe, { uefiOverride: "1" });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.args.some((arg) => /--field=(UEFI|Driver) version:CB/.test(arg)), false);
  });

  it("shows driver versions as well when latest drivers is unchecked", () => {
    const result = run(`DRIVERS_USE_LATEST=0\n${linuxProbe}`, { model: 3 });
    assert.equal(result.status, 0, result.stderr);
    const index = result.args.indexOf("--field=Driver version:CB");
    assert.ok(index >= 0);
    assert.equal(result.args[index + 1], "v0.17 (latest) [recommended]!v0.16");
  });

  it("saves the exact versions returned by the macOS dropdowns", () => {
    const probe = macosProbe.replace(
      '"$uefi_dropdown_default" "$driver_dropdown_default"\n  fi',
      '"v1.20" "v0.16"\n  fi',
    );
    assert.notEqual(probe, macosProbe);
    const result = run(`DRIVERS_USE_LATEST=0\n${probe}\nprintf '%s|%s' "$(uefi_pinned_version)" "$DRIVER_VER" > "$TEST_DIR/release-state"`, { model: 3 });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.releaseState, "v1.20|v0.16");
    assert.match(result.summary, /UEFI firmware\tPinned \(v1\.20\)/);
    assert.match(result.summary, /Windows ARM64 drivers\tPinned \(v0\.16\)/);
  });

  it("saves the exact versions returned by the Linux dropdowns", () => {
    const probe = linuxProbe.replace(
      '    case "$label" in',
      '    case "$label" in\n      *"UEFI version:CB") value=v1.20 ;;\n      *"Driver version:CB") value=v0.16 ;;',
    );
    const result = run(`DRIVERS_USE_LATEST=0\n${probe}\nprintf '%s|%s' "$(uefi_pinned_version)" "$DRIVER_VER" > "$TEST_DIR/release-state"`, { model: 3 });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.releaseState, "v1.20|v0.16");
    assert.match(result.summary, /UEFI firmware\tPinned \(v1\.20\)/);
    assert.match(result.summary, /Windows ARM64 drivers\tPinned \(v0\.16\)/);
  });

  it("retains the current choices and displays a warning when GitHub is unavailable", () => {
    const result = run(`
      list_release_versions() { printf 'GitHub is unavailable.\\n' >&2; return 1; }
      ${macosProbe}
    `, { model: 3 });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.args[25], "v1.39");
    assert.equal(result.args[27], "v0.17");
    assert.match(result.args[29], /GitHub is unavailable.*Only the current selection/);
    assert.match(result.args[30], /GitHub is unavailable.*Only the current selection/);
  });

  it("preserves all edits while Linux refreshes, then cancels without committing them", () => {
    const refreshProbe = linuxProbe.replace(
      '  [ "$TEST_CANCEL" != 1 ] || return 1',
      `  if [ -f "$TEST_DIR/refreshed" ];then return 1;fi
  touch "$TEST_DIR/refreshed"
  printf 'refresh\\n' > "$release_refresh_file"`,
    );
    const result = run(`${refreshProbe}\nprintf '%s|%s|%s|%s' "$WINDOWS_ACCOUNT_USERNAME" "$WINDOWS_LOCALE" "$(uefi_pinned_version)" "$DRIVER_VER" > "$TEST_DIR/release-state"`, { model: 3, edit: true });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.state.trim(), "1|1|0|0|0|en-us|1|en-US|1");
    assert.equal(result.releaseState, "fixture-user|en-US|v1.39|v0.17");
    assert.equal(result.releaseFlags, "");
    assert.ok(result.args.includes("--response=0"));
    assert.match(result.args.find((arg) => arg.startsWith("--changed-action=")), /kill -USR1 "\$YAD_PID"/);
  });

  it("keeps disabled account and locale values through an automatic Linux refresh", () => {
    const refreshProbe = linuxProbe.replace(
      '  [ "$TEST_CANCEL" != 1 ] || return 1',
      `  [ "$TEST_CANCEL" != 1 ] || return 1
  if [ ! -f "$TEST_DIR/refreshed" ];then
    touch "$TEST_DIR/refreshed"
    printf 'refresh\\n' > "$release_refresh_file"
  fi`,
    );
    const result = run(`WINDOWS_LOCALE_SETUP=0\n${refreshProbe}\nprintf '%s|%s|%s' "$WINDOWS_ACCOUNT_USERNAME" "$WINDOWS_ACCOUNT_PASSWORD" "$WINDOWS_LOCALE" > "$TEST_DIR/release-state"`);
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.releaseState, "fixture-user|fixture-password|en-US");
  });
});

describe("Recommended latest release dropdown defaults", () => {
  const catalogue = `
    list_release_versions() {
      case "$1:\${2:-all}" in
        uefi:all) printf 'v1.54\\nv1.53.1\\nv1.50\\nv1.39\\nv0.3\\n' ;;
        uefi:latest) printf 'v1.53.1\\n' ;;
        drivers:all) printf 'v0.19\\nv0.18\\nv0.17\\nv0.16\\n' ;;
        drivers:latest) printf 'v0.18\\n' ;;
      esac
    }
  `;

  it("preselects the authoritative latest Pi 3 UEFI tag rather than the retained pin or first release", () => {
    const result = run(`${catalogue}\n${macosProbe}`, { model: 3, uefiOverride: "1" });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.args[26], "v1.53.1");
    assert.equal(result.args[28], "v0.18");
    assert.equal(result.args[31].split("\n")[0], "v1.53.1 (latest) [recommended]");
    assert.equal(result.args[32].split("\n")[0], "v0.18 (latest) [recommended]");
    assert.match(result.summary, /UEFI firmware\tLatest/);
  });

  it("keeps Pi 4/5 UEFI pins as their dropdown defaults", () => {
    for (const model of [4, 5]) {
      const result = run(`${catalogue}\n${macosProbe}`, { model });
      assert.equal(result.status, 0, result.stderr);
      assert.equal(result.args[26], model === 4 ? "v1.50" : "v0.3");
      assert.doesNotMatch(result.args[31], /\[recommended\]/);
    }
  });

  it("does not update the fallback versions merely by accepting latest mode", () => {
    const result = run(`${catalogue}\n${macosProbe}
      printf '%s|%s' "$(uefi_pinned_version)" "$DRIVER_VER" > "$TEST_DIR/release-state"
    `, { model: 3, uefiOverride: "1" });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.releaseState, "v1.39|v0.17");
    assert.equal(result.releaseFlags, "");
  });

  it("pins the recommended raw tags when latest is unchecked on macOS", () => {
    const result = run(`${catalogue}\n${macosProbe}
      printf '%s|%s' "$(uefi_pinned_version)" "$DRIVER_VER" > "$TEST_DIR/release-state"
    `, { model: 3, uefiOverride: "1", uefiSelection: "0", driverSelection: "0" });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.releaseState, "v1.53.1|v0.18");
    assert.match(result.summary, /UEFI firmware\tPinned \(v1\.53\.1\)/);
    assert.match(result.summary, /Windows ARM64 drivers\tPinned \(v0\.18\)/);
    assert.doesNotMatch(result.summary, /\[recommended\]/);
  });

  const uncheckLinux = linuxProbe.replace(
    '  [ "$TEST_CANCEL" != 1 ] || return 1',
    `  local uncheck=0
  if [ ! -f "$TEST_DIR/unchecked" ];then
    uncheck=1
    touch "$TEST_DIR/unchecked"
    printf 'refresh\\n' > "$release_refresh_file"
  fi`,
  ).replace(
    '    printf \'%s\\n\' "\${value%%!*}"',
    `    if [ "$uncheck" == 1 ] && { [[ "$label" == *'latest UEFI'* ]] || [[ "$label" == *'latest Windows ARM64 drivers'* ]]; };then value=FALSE;fi
    printf '%s\\n' "\${value%%!*}"`,
  );
  assert.notEqual(uncheckLinux, linuxProbe);

  it("preselects labeled latest entries after Linux's automatic uncheck/refresh", () => {
    const result = run(`${catalogue}\n${uncheckLinux}
      printf '%s|%s' "$(uefi_pinned_version)" "$DRIVER_VER" > "$TEST_DIR/release-state"
    `, { model: 3, uefiOverride: "1" });
    assert.equal(result.status, 0, result.stderr);
    for (const [field, tag] of [["UEFI", "v1.53.1"], ["Driver", "v0.18"]]) {
      const index = result.args.indexOf(`--field=${field} version:CB`);
      assert.ok(index >= 0);
      assert.equal(result.args[index + 1].split("!")[0], `${tag} (latest) [recommended]`);
    }
    assert.equal(result.releaseState, "v1.53.1|v0.18");
    assert.match(result.summary, /UEFI firmware\tPinned \(v1\.53\.1\)/);
  });

  it("reverts the automatically chosen latest tags when Linux Back is clicked", () => {
    const cancelProbe = uncheckLinux.replace(
      '  local uncheck=0',
      '  [ ! -f "$TEST_DIR/unchecked" ] || return 1\n  local uncheck=0',
    );
    const result = run(`${catalogue}\n${cancelProbe}
      printf '%s|%s' "$(uefi_pinned_version)" "$DRIVER_VER" > "$TEST_DIR/release-state"
    `, { model: 3, uefiOverride: "1" });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.releaseState, "v1.39|v0.17");
    assert.equal(result.state.split("|")[1], "1");
    assert.equal(result.state.split("|")[2], "1");
    assert.equal(result.releaseFlags, "");
  });

  it("retains deliberate older choices after re-enabling latest", () => {
    const result = run(`${catalogue}
      set_selected_release_version uefi v1.39
      set_selected_release_version drivers v0.16
      ${macosProbe}
    `, { model: 3, uefiOverride: "1" });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.args[26], "v1.39");
    assert.equal(result.args[28], "v0.16");
    assert.ok(result.args[31].includes("v1.53.1 (latest) [recommended]"));
    assert.ok(result.args[32].includes("v0.18 (latest) [recommended]"));
  });

  it("retains explicitly configured pins while the latest boxes are already unchecked", () => {
    const result = run(`${catalogue}\nDRIVERS_USE_LATEST=0\n${macosProbe}`, { model: 3 });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.args[26], "v1.39");
    assert.equal(result.args[28], "v0.17");
  });

  it("does not replace a custom fallback tag or a driver choice made for another supported model", () => {
    const result = run(`${catalogue}
      UEFI_VER_PI3=v1.50
      RPI_MODEL=4
      set_selected_release_version drivers v0.17
      RPI_MODEL=3
      ${macosProbe}
    `, { model: 3, uefiOverride: "1" });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.args[26], "v1.50");
    assert.equal(result.args[28], "v0.17");
  });

  it("does not label a fallback as latest when GitHub latest lookup fails", () => {
    const result = run(`
      list_release_versions() {
        if [ "\${2:-all}" == latest ];then printf 'Latest lookup failed.\\n' >&2; return 1;fi
        printf '%s\\n' v1.39 v0.17
      }
      ${macosProbe}
    `, { model: 3, uefiOverride: "1" });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.args[26], "v1.39");
    assert.equal(result.args[28], "v0.17");
    assert.doesNotMatch(result.args[31] + result.args[32], /\(latest\)|\[recommended\]/);
    assert.match(result.args[29] + result.args[30], /Latest lookup failed/);
  });

  for (const label of [
    "v0.18 (latest) [recommended]",
    "v0.16",
    "v0.16 (latest) [recommended]",
    "v0.18 (latest) [recommended] unexpected",
  ]) {
    it(`validates display label ${JSON.stringify(label)} before returning a raw tag`, () => {
      const result = run(`release_choice_tag '${label}' $'v0.18\\nv0.16' v0.18`);
      if (label === "v0.18 (latest) [recommended]" || label === "v0.16") {
        assert.equal(result.status, 0, result.stderr);
        assert.equal(result.stdout, label.split(" ")[0]);
      } else {
        assert.notEqual(result.status, 0);
        assert.equal(result.stdout, "");
        assert.match(result.stderr, /not in the available choices/);
      }
    });
  }
});

describe("Windows 11-only offline setup bypass", () => {
  for (const [frontend, probe] of [["macOS", macosProbe], ["Linux", linuxProbe]]) {
    it(`${frontend} ignores edits to the hidden Windows 10 bypass`, () => {
      const result = run(probe, { version: 10, edit: true });
      assert.equal(result.status, 0, result.stderr);
      assert.equal(result.oobePreference, "1");
      assert.doesNotMatch(result.summary, /Offline OOBE/);
    });

    for (const preference of ["0", "1"]) {
      it(`${frontend} preserves preference ${preference} when switching Windows 10 back to 11`, () => {
        const result = run(`${probe}\nBID=22631.2861\n${probe}`, { version: 10, oobeOverride: preference });
        assert.equal(result.status, 0, result.stderr);
        assert.equal(result.oobePreference, preference);
        assert.match(result.summary, new RegExp(`Offline OOBE\\t${preference === "1" ? "Allowed" : "Disabled"}`));
      });
    }
  }

  for (const version of [10, 11]) {
    for (const mode of [0, 1]) {
      it(`emits only the applicable setup XML for Windows ${version} in mode ${mode}`, () => {
        const result = run(`
          WINDOWS_ACCOUNT_SETUP=1 WINDOWS_LOCALE_SETUP=1 PI4_AUTO_DISABLE_3GB=0
          unattend_xml > "$TEST_DIR/answer.xml" || exit 91
          is_macos() { return 0; }
          mkdir -p "$TEST_DIR/boot" "$TEST_DIR/install" peinstaller/winpe/2
          printf 'fixture\\n' > peinstaller/winpe/2/setup.exe
          mark_cache "$PWD/peinstaller" fixture
          install_windows_setup_configuration "$TEST_DIR/boot" "$TEST_DIR/install" || exit 92
          configure_pe_prefinalize || exit 93
          cmp "$TEST_DIR/answer.xml" "$TEST_DIR/boot/Autounattend.xml" || exit 94
          cmp "$TEST_DIR/answer.xml" "$TEST_DIR/install/Autounattend.xml" || exit 95
          cmp "$TEST_DIR/answer.xml" peinstaller/winpe/2/scripts/unattend.xml || exit 96
          cache_is_current "$PWD/peinstaller" fixture || exit 97
        `, { version, mode, model: 3 });
        assert.equal(result.status, 0, result.stderr);
        assert.equal(result.answer.includes("<HideOnlineAccountScreens>"), version === 11);
        assert.equal(result.answer.includes("<HideWirelessSetupInOOBE>"), version === 11);
        assert.match(result.answer, /<LocalAccount wcm:action="add">/);
        assert.match(result.answer, /<InputLocale>en-US<\/InputLocale>/);
      });
    }
  }

  it("drops a cached Windows 11 answer file but keeps mandatory Pi 3 boot finalization on Windows 10", () => {
    const result = run(`
      WINDOWS_ACCOUNT_SETUP=0 WINDOWS_LOCALE_SETUP=0 PI4_AUTO_DISABLE_3GB=0 PI4_UEFI_SHELL_UNLOCK=0
      mkdir -p peinstaller/winpe/2
      printf 'fixture\\n' > peinstaller/winpe/2/setup.exe
      mark_cache "$PWD/peinstaller" fixture
      configure_pe_prefinalize || exit 91
      grep -q 'HideOnlineAccountScreens' peinstaller/winpe/2/scripts/unattend.xml || exit 92
      BID=19045.3803
      if windows_setup_configuration_enabled;then exit 93;fi
      if unattend_xml > "$TEST_DIR/answer.xml";then exit 94;fi
      configure_pe_prefinalize || exit 95
      [ ! -e peinstaller/winpe/2/scripts/unattend.xml ] &&
        [ -s peinstaller/winpe/2/scripts/Pi3BootRefresh.exe ] || exit 96
      cache_is_current "$PWD/peinstaller" fixture || exit 97
    `, { version: 11, model: 3 });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.answer, "");
  });

  it("keeps the Windows 11 bypass optional and follows the build rather than a stale wizard choice", () => {
    const result = run(`
      OOBE_NETWORK_BYPASS=0 WINDOWS_ACCOUNT_SETUP=0 WINDOWS_LOCALE_SETUP=1 PI4_AUTO_DISABLE_3GB=0
      unattend_xml > "$TEST_DIR/answer.xml" || exit 91
      if windows_oobe_bypass_enabled;then exit 92;fi
      OOBE_NETWORK_BYPASS=1 WINDOWS_VER="Windows 10"
      windows_oobe_bypass_enabled || exit 93
      BID=19045.3803 WINDOWS_VER="Windows 11"
      if advanced_option_applies oobe;then exit 94;fi
    `, { version: 11, model: 3 });
    assert.equal(result.status, 0, result.stderr);
    assert.doesNotMatch(result.answer, /HideOnlineAccountScreens|HideWirelessSetupInOOBE/);
    assert.match(result.answer, /<InputLocale>en-US<\/InputLocale>/);
  });
});

describe("Adaptive fields preserve preferences and field positions", () => {
  for (const model of [3, 4, 5]) {
    it(`parses the remaining controls correctly on Pi ${model}`, () => {
      const mac = run(macosProbe, { model, edit: true });
      const lin = run(linuxProbe, { model, edit: true });
      assert.equal(mac.status, 0, mac.stderr);
      assert.equal(lin.status, 0, lin.stderr);
      const expected = `${model === 4 ? 0 : 1}|${model === 5 ? 1 : 0}|1|1|1|fr-fr|2|fr-FR|0`;
      assert.equal(mac.state.trim(), `${expected}|edited-config`);
      assert.equal(lin.state.trim(), expected);
    });
  }

  for (const iso of ["source", "cached"]) {
    it(`omits image-language selection for ${iso} ISOs and ignores hidden language changes`, () => {
      const mac = run(macosProbe, { iso, edit: true });
      const lin = run(linuxProbe, { iso, edit: true });
      assert.equal(mac.status, 0, mac.stderr);
      assert.equal(lin.status, 0, lin.stderr);
      assert.equal(mac.args[13], "");
      assert.equal(lin.args.some((value) => value.startsWith("--field=Choose Windows language")), false);
      assert.equal(mac.state.split("|")[5], "en-us");
      assert.equal(lin.state.split("|")[5], "en-us");
      assert.equal(mac.state.split("|")[7], "fr-FR");
      assert.equal(lin.state.split("|")[7], "fr-FR");
    });
  }

  it("does not modify settings when the macOS dialog is canceled", () => {
    const result = run(macosProbe, { model: 5, cancel: true });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.state.trim(), "1|1|0|0|0|en-us|1|en-US|1|fixture-config");
  });

  it("uses the selected build rather than a stale wizard family", () => {
    const result = run('WINDOWS_VER="Windows 11"; windows_version_label', { version: 10 });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, "Windows 10");
  });

  it("refreshes Linux overview values after returning from Advanced Options", () => {
    const loop = gui.indexOf("while true;do #repeat the Installation Overview window");
    const summary = gui.indexOf('window_text="$(settings_summary_markup)', loop);
    const display = gui.indexOf('--field="$window_text":LBL', loop);
    assert.ok(loop >= 0 && loop < summary && summary < display);
  });
});

describe("Setup personalization applies to both Windows versions and installation modes", () => {
  for (const version of [10, 11]) {
    for (const mode of [0, 1]) {
      for (const option of ["account", "locale"]) {
        it(`stages ${option}-only Windows ${version} settings in mode ${mode}`, () => {
          const result = run(`
            OOBE_NETWORK_BYPASS=0 PI4_AUTO_DISABLE_3GB=0 WINDOWS_ACCOUNT_SETUP=0 WINDOWS_LOCALE_SETUP=0
            ${option === "account" ? "WINDOWS_ACCOUNT_SETUP=1" : "WINDOWS_LOCALE_SETUP=1"}
            is_macos() { return 0; }
            mkdir -p "$TEST_DIR/boot" "$TEST_DIR/install" peinstaller/winpe/2
            printf 'fixture\\n' > peinstaller/winpe/2/setup.exe
            mark_cache "$PWD/peinstaller" fixture
            install_windows_setup_configuration "$TEST_DIR/boot" "$TEST_DIR/install" || exit 91
            configure_pe_prefinalize || exit 92
            cmp "$TEST_DIR/boot/Autounattend.xml" "$TEST_DIR/install/Autounattend.xml" || exit 93
            cmp "$TEST_DIR/boot/Autounattend.xml" peinstaller/winpe/2/scripts/unattend.xml || exit 94
            grep -q '${option === "account" ? "LocalAccount" : "InputLocale"}' "$TEST_DIR/boot/Autounattend.xml" || exit 95
            ! grep -q 'HideWirelessSetupInOOBE\\|WillReboot' "$TEST_DIR/boot/Autounattend.xml" || exit 96
            cache_is_current "$PWD/peinstaller" fixture || exit 97
            WINDOWS_ACCOUNT_SETUP=0 WINDOWS_LOCALE_SETUP=0
            configure_pe_prefinalize || exit 98
            [ ! -e peinstaller/winpe/2/scripts/unattend.xml ] &&
              [ -s peinstaller/winpe/2/scripts/Pi3BootRefresh.exe ] || exit 99
          `, { version, mode, model: 3 });
          assert.equal(result.status, 0, result.stdout + result.stderr);
          assert.doesNotMatch(result.stdout + result.stderr, /fixture-password/);
        });
      }
    }
  }

  it("does not emit Pi 4 settings for hidden preferences on other models", () => {
    const result = run(`
      RPI_MODEL=3 OOBE_NETWORK_BYPASS=0 WINDOWS_ACCOUNT_SETUP=0 WINDOWS_LOCALE_SETUP=0 PI4_AUTO_DISABLE_3GB=1
      if unattend_xml;then exit 91;fi
      RPI_MODEL=4
      unattend_xml | grep -q 'Pi4Disable3GB.ps1' || exit 92
      RPI_MODEL=5
      if unattend_xml;then exit 93;fi
      [ "$PI4_AUTO_DISABLE_3GB" == 1 ]
    `);
    assert.equal(result.status, 0, result.stderr);
  });
});

        describe("Model-specific UEFI defaults and explicit overrides", () => {
          for (const model of [3, 4, 5]) {
            it(`starts with the correct firmware default on Pi ${model}`, () => {
              const result = run(`
                unset UEFI_USE_LATEST
                source "$TEST_ROOT/install-wor.sh" source >/dev/null || exit 90
                settings_summary
              `, { model });
              assert.equal(result.status, 0, result.stderr);
              assert.match(result.stdout, model === 3 ? /UEFI firmware\tLatest\n/ : /UEFI firmware\tPinned \(/);
            });

            for (const [frontend, probe] of [["macOS", macosProbe], ["Linux", linuxProbe]]) {
              it(`${frontend} shows the Pi ${model} default without persisting an unchanged checkbox`, () => {
                const result = run(probe, { model, uefiOverride: "" });
                assert.equal(result.status, 0, result.stderr);
                assert.equal(result.state.split("|")[2], "");
                assert.match(result.summary, model === 3 ? /UEFI firmware\tLatest\n/ : /UEFI firmware\tPinned \(/);
                if (frontend === "macOS") {
                  const row = result.args[3].split("\n")[2].split("\t");
                  assert.equal(row[1], model === 3 ? "1" : "0");
                  assert.equal(row[3], model === 3 ? "0" : "1");
                  assert.equal(row[4], model === 3 ? "1" : "0");
                  const rendered = renderedMacCheckboxes(result.args[3], result.args[24]);
                  const control = rendered.controls.find((item) => item.label.includes("latest UEFI"));
                  assert.ok(control);
                  assert.equal(control.state, model === 3 ? 1 : 0);
                  assert.deepEqual(rendered.annotations.find((item) => item.label === control.label), {
                    label: control.label, note: model === 3 ? "Recommended" : "Not recommended",
                    color: model === 3 ? "green" : "red",
                  });
                  assert.equal(row[0], "Use the latest UEFI firmware");
                } else {
                  const fieldIndex = result.args.findIndex((value) => value.startsWith("--field=") && value.includes("latest UEFI"));
                  assert.ok(fieldIndex >= 0);
                  assert.equal(result.args[fieldIndex + 1], model === 3 ? "TRUE" : "FALSE");
                  assert.equal(result.args[fieldIndex].includes("not recommended"), model !== 3);
                  assert.equal(result.args[fieldIndex].includes("(recommended)"), model === 3);
                }
              });
            }
          }

          it("keeps following the model when unchanged choices are accepted", () => {
            const result = run(`
              unset UEFI_USE_LATEST
              for RPI_MODEL in 3 4 5 3;do
                printf '%s\\n' "$(uefi_use_latest)"
                set_uefi_use_latest_choice "$(uefi_use_latest)" || exit 91
              done
              [ -z "\${UEFI_USE_LATEST:-}" ]
            `);
            assert.equal(result.status, 0, result.stderr);
            assert.equal(result.stdout, "1\n0\n0\n1\n");
          });

          for (const [frontend, probe] of [["macOS", macosProbe], ["Linux", linuxProbe]]) {
            it(`${frontend} retains an explicit pinned choice on Pi 3`, () => {
              const result = run(probe, { model: 3, uefiOverride: "", uefiSelection: "0" });
              assert.equal(result.status, 0, result.stderr);
              assert.equal(result.state.split("|")[2], "0");
              assert.match(result.summary, /UEFI firmware\tPinned \(v1\.39\)/);
            });
          }

          const configs = [
            { value: { customization: { uefiUseLatest: false } }, expected: "0" },
            { value: { customization: { uefiUseLatest: true } }, expected: "1" },
            { value: { uefiUseLatest: false }, expected: "0" },
            { value: { UEFI_USE_LATEST: false }, expected: "0" },
            { value: { customization: { uefiUseLatest: false }, uefiUseLatest: true }, expected: "0" },
            { value: {}, expected: "1" },
          ];
          for (const { value, expected } of configs) {
            it(`honors configuration ${JSON.stringify(value)}`, () => {
              const result = run(`
                printf '%s\\n' '${JSON.stringify(value)}' > "$TEST_DIR/firmware.json"
                unset UEFI_USE_LATEST
                load_config_json "$TEST_DIR/firmware.json"
                uefi_use_latest
              `, { model: 3 });
              assert.equal(result.status, 0, result.stderr);
              assert.equal(result.stdout, expected);
            });
          }

          it("gives an explicit environment preference precedence over configuration", () => {
            const result = run(`
              printf '%s\\n' '{"customization":{"uefiUseLatest":true}}' > "$TEST_DIR/firmware.json"
              UEFI_USE_LATEST=0
              load_config_json "$TEST_DIR/firmware.json"
              uefi_use_latest
            `, { model: 3 });
            assert.equal(result.status, 0, result.stderr);
            assert.equal(result.stdout, "0");
          });

          it("rejects an invalid explicit environment choice before preparation", () => {
            const result = run('UEFI_USE_LATEST=invalid; source "$TEST_ROOT/install-wor.sh" source');
            assert.notEqual(result.status, 0);
            assert.match(result.stderr, /Unknown value for UEFI_USE_LATEST/);
          });

          it("carries automatic model selection into the installer process", () => {
            const result = run(`
              unset UEFI_USE_LATEST
              export_installer_settings
              env | grep -qx 'UEFI_USE_LATEST=' || exit 91
              bash -c 'source "$DIRECTORY/install-wor.sh" source >/dev/null; uefi_use_latest'
            `, { model: 3 });
            assert.equal(result.status, 0, result.stderr);
            assert.equal(result.stdout, "1");
          });
        });

        describe("Pi 3 UEFI recommendation preserves deliberate pinning", () => {
          it("keeps the green recommendation without overriding an explicit unchecked preference", () => {
            const result = run(macosProbe, { model: 3, uefiOverride: "0" });
            assert.equal(result.status, 0, result.stderr);
            const rendered = renderedMacCheckboxes(result.args[3], result.args[24]);
            const control = rendered.controls.find((item) => item.label.includes("latest UEFI"));
            assert.ok(control);
            assert.equal(control.state, 0);
            assert.deepEqual(rendered.annotations.find((item) => item.label === control.label), {
              label: control.label, note: "Recommended", color: "green",
            });
            assert.match(result.summary, /UEFI firmware\tPinned \(v1\.39\)/);
          });
        });

        describe("UEFI runtime selection and fallback", () => {
          function prepare({ model = 3, override = "", apiFails = false, trustCache = false } = {}) {
            return run(`
              USE_CACHE=${trustCache ? 2 : 1}
              ${trustCache ? 'mkdir -p "pi${RPI_MODEL}-uefipackage"' : ""}
              phase() { :; }
              wget() {
                [ "$1" == -qO- ] || { printf 'Unexpected download attempt\\n' >&2; return 99; }
                printf 'LOOKUP %s\\n' "$2" >> "$TEST_DIR/firmware-requests"
                ${apiFails ? "return 1" : 'printf \'  "browser_download_url": "https://example.invalid/RPi%s_UEFI_Firmware_v9.9.zip"\\n\' "$RPI_MODEL"'}
              }
              cache_is_current() { printf 'SELECTED %s\\n' "$2"; return 0; }
              ${firmware}
              [ ! -f "$TEST_DIR/firmware-requests" ] || cat "$TEST_DIR/firmware-requests"
            `, { model, uefiOverride: override });
          }

          it("queries the latest Pi 3 release by default", () => {
            const result = prepare();
            assert.equal(result.status, 0, result.stderr);
            assert.match(result.stdout, /SELECTED https:\/\/example\.invalid\/RPi3_UEFI_Firmware_v9\.9\.zip/);
            assert.match(result.stdout, /LOOKUP https:\/\/api\.github\.com\/repos\/pftf\/RPi3\/releases\/latest/);
          });

          for (const model of [4, 5]) {
            it(`uses the Pi ${model} pin without a latest lookup by default`, () => {
              const result = prepare({ model });
              assert.equal(result.status, 0, result.stderr);
              assert.match(result.stdout, model === 4 ? /releases\/download\/v1\.50\// : /releases\/download\/v0\.3\//);
              assert.doesNotMatch(result.stdout, /example\.invalid/);
              assert.doesNotMatch(result.stdout, /LOOKUP/);
            });
          }

          it("honors an explicit Pi 3 pin without a lookup", () => {
            const result = prepare({ override: "0" });
            assert.equal(result.status, 0, result.stderr);
            assert.match(result.stdout, /releases\/download\/v1\.39\//);
            assert.doesNotMatch(result.stdout, /example\.invalid/);
            assert.doesNotMatch(result.stdout, /LOOKUP/);
          });

          it("honors an explicit latest override on Pi 4", () => {
            const result = prepare({ model: 4, override: "1" });
            assert.equal(result.status, 0, result.stderr);
            assert.match(result.stdout, /SELECTED https:\/\/example\.invalid\/RPi4_UEFI_Firmware_v9\.9\.zip/);
          });

          it("reports lookup failure and uses the Pi 3 fallback pin", () => {
            const result = prepare({ apiFails: true });
            assert.equal(result.status, 0, result.stderr);
            assert.match(result.stderr, /Falling back to pinned version v1\.39/);
            assert.match(result.stdout, /SELECTED .*releases\/download\/v1\.39\//);
          });

          it("preserves the explicit trust-cache policy", () => {
            const result = prepare({ trustCache: true });
            assert.equal(result.status, 0, result.stderr);
            assert.match(result.stdout, /using cache without checking for updates/);
            assert.doesNotMatch(result.stdout, /SELECTED|LOOKUP/);
          });
        });
