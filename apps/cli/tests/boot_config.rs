use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::OnceLock;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_wor-flasher")
}

fn invoke(mut command: Command, input: &[u8]) -> Output {
    let mut child = command
        .current_dir(root())
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start boot-config implementation");
    child.stdin.take().unwrap().write_all(input).unwrap();
    child.wait_with_output().unwrap()
}

fn native(input: &[u8]) -> Output {
    let mut command = Command::new(binary());
    command.arg("boot-config");
    invoke(command, input)
}

fn oracle(input: &[u8]) -> Output {
    let mut command = Command::new("python3");
    command.arg(root().join("src/lib/boot-config.py"));
    invoke(command, input)
}

fn contexts() -> &'static BTreeMap<&'static str, Value> {
    static CONTEXTS: OnceLock<BTreeMap<&'static str, Value>> = OnceLock::new();
    CONTEXTS.get_or_init(|| {
        ["3", "4", "5"]
            .into_iter()
            .map(|model| {
                let output = engine(
                    "RPI_MODEL=\"$TEST_MODEL\"; boot_config_context",
                    &[("TEST_MODEL", model)],
                    false,
                );
                assert!(output.status.success(), "{output:?}");
                let mut request: Value = serde_json::from_slice(&output.stdout).unwrap();
                request["config"] = request["recommended"].clone();
                (model, request)
            })
            .collect()
    })
}

fn engine(body: &str, variables: &[(&str, &str)], select_native: bool) -> Output {
    let mut command = Command::new("bash");
    command
        .arg("-c")
        .arg(format!(
            "source \"$DIRECTORY/install-wor.sh\" source >/dev/null || exit 90\n{body}"
        ))
        .env("DIRECTORY", root())
        .env("NO_UPDATE", "1")
        .env("BOOT_CONFIG_SETTINGS", "")
        .env_remove("WOR_RUST_BOOT_CONFIG");
    if select_native {
        command.env("WOR_RUST_BOOT_CONFIG", binary());
    }
    for (key, value) in variables {
        command.env(key, value);
    }
    invoke(command, b"")
}

fn with(base: &Value, changes: Value) -> Value {
    let mut request = base.clone();
    request
        .as_object_mut()
        .unwrap()
        .extend(changes.as_object().unwrap().clone());
    request
}

fn compare(label: &str, input: &[u8]) -> Value {
    let expected = oracle(input);
    let actual = native(input);
    assert_eq!(
        actual.status.code(),
        expected.status.code(),
        "{label}: expected {expected:?}, actual {actual:?}"
    );
    let expected: Value = serde_json::from_slice(&expected.stdout).unwrap_or_else(|error| {
        panic!("{label}: oracle did not return JSON: {error}; {expected:?}")
    });
    let actual: Value = serde_json::from_slice(&actual.stdout)
        .unwrap_or_else(|error| panic!("{label}: native did not return JSON: {error}; {actual:?}"));
    assert_eq!(actual, expected, "{label}");
    actual
}

fn corpus() -> Vec<(String, Value)> {
    let mut cases = Vec::new();
    for model in ["3", "4", "5"] {
        let base = &contexts()[model];
        for changes in [
            json!({}),
            json!({"config": base["official"]}),
            json!({"reset": true}),
            json!({"hdmiMode": "official"}),
            json!({"hdmiMode": "recommended"}),
            json!({"hdmiMode": "custom"}),
            json!({"values": {"enable_uart": "0"}}),
            json!({"userText": "# hardware\ndtparam=i2c_arm=on\ngpio=2=op,dh\n"}),
            json!({"userText": "hdmi_group=1\nhdmi_mode=16\nhdmi_force_hotplug=1\n"}),
        ] {
            cases.push((format!("Pi {model}: {changes}"), with(base, changes)));
        }
    }
    let base = &contexts()["4"];
    let official = base["official"].as_str().unwrap();
    for additions in [
        "\nenable_uart=0 # old\nenable_uart=0x1 # current\n",
        "\nfuture_option=first\nfuture_option=second\n",
        "\ndtoverlay=custom-overlay\ndtoverlay=miniuart-bt\n\
         dtparam=i2c_arm=on\ndtparam=audio=on\ngpio=2=op,dh\ngpio=3=op,dl\n",
        "\n[pi4]\nenable_uart=0\n[all]\nenable_uart=1\n",
        "\nhdmi_drive:0=1\nhdmi_drive=2\n",
        "\nhdmi_drive=2\n[HDMI:0]\nhdmi_drive=1\n[all]\nhdmi_drive=2\n",
        "\nhdmi_group=2\nhdmi_mode=87\n",
        "\nhdmi_group=2\nhdmi_mode=0x57\n",
        "\nhdmi_group=2\nhdmi_mode=0x057\n",
        "\nhdmi_group=2\nhdmi_mode=87\nhdmi_cvt=1920 1080 60\n",
        "\n[pi4]\nhdmi_group=2\nhdmi_mode=87\n[all]\nhdmi_cvt=1920 1080 60 3 0 0 1\n",
        "\nhdmi_ignore_hotplug=1\n",
        "\ncore_freq=0xFA\narm_freq=01200\ngpu_mem=64\nboot_delay=0\nboot_delay_ms=1\n",
        "\nboot_delay=99999999999999999999999999999999999999999999999999999999999999\n",
        "\nunknown=$(touch must-not-exist)\n",
        "\n# Unicode comment: café 🦀\n",
        "\n# ",
        "\n# comment\u{0085}custom_option=keep\u{2028}# another\u{2029}",
        "\ncustom_option:0=keep\n",
        "\nENABLE_UART=anything\n",
    ] {
        cases.push((
            format!("preserved additions {additions:?}"),
            with(base, json!({"config": format!("{official}{additions}")})),
        ));
    }
    for suffix in [
        "enable_uart=2",
        "enable_uart=true",
        "enable_uart=-1",
        "enable_uart=0X1",
        "enable_uart=",
        "hdmi_group=3",
        "hdmi_drive=9",
        "hdmi_mode=108",
        "hdmi_group=2\nhdmi_mode=88",
        "hdmi_cvt=1920 nope 60",
        "hdmi_cvt=1920 1080",
        "hdmi_cvt=1920 1080 60 6 0 0 0 0",
        "hdmi_cvt=0 1080 60",
        "hdmi_cvt=1920 0 60",
        "hdmi_cvt=1920 1080 0",
        "hdmi_cvt=1920 1080 60 0",
        "hdmi_cvt=1920 1080 60 7",
        "hdmi_cvt=1920 1080 60 6 2",
        "hdmi_cvt=1920 1080 60 6 0 2",
        "hdmi_cvt=1920 1080 60 6 0 0 2",
        "arm_freq=nope",
        "[pi4",
        "[]",
        "[[pi4]]",
        "not a setting",
        "include other.txt",
        "include\tother.txt",
        "unknown=a\0b",
        "unknown=café",
        "hdmi_drive:2=1",
        "_key=1",
        "1key=1",
    ] {
        cases.push((
            format!("invalid addition {suffix:?}"),
            with(base, json!({"config": format!("{official}\n{suffix}\n")})),
        ));
    }
    for key in [
        "arm_64bit",
        "armstub",
        "enable_gic",
        "device_tree_address",
        "device_tree_end",
        "disable_commandline_tags",
    ] {
        for section in ["[pi4]", "[all]"] {
            cases.push((
                format!("protected {key} in {section}"),
                with(
                    base,
                    json!({"config": format!("{official}\n{section}\n{key}=0\n")}),
                ),
            ));
        }
        cases.push((
            format!("user additions cannot own {key}"),
            with(base, json!({"userText": format!("{key}=0\n")})),
        ));
    }
    for changes in [
        json!({"profile": "unknown"}),
        json!({"profile": null}),
        json!({"model": "2"}),
        json!({"model": 4}),
        json!({"model": null}),
        json!({"official": null}),
        json!({"official": ""}),
        json!({"recommended": null}),
        json!({"config": null}),
        json!({"config": 123}),
        json!({"config": "#".repeat(65537)}),
        json!({"config": format!("{official}\nunknown={}", "a".repeat(99))}),
        json!({"config": format!("\u{feff}{official}")}),
        json!({"config": official.replace('\n', "\r\n")}),
        json!({"config": official.replace('\n', "\r")}),
        json!({"config": official.replace("arm_64bit=1", "arm_64bit=0x1")}),
        json!({"config": official.replace("armstub=RPI_EFI.fd", "")}),
        json!({"config": official.replace("dtoverlay=upstream-pi4", "")}),
        json!({"config": format!("# >>> WoR-Flasher managed boot settings >>>\n{official}")}),
        json!({"config": format!("# <<< WoR-Flasher managed boot settings <<<\n# >>> WoR-Flasher managed boot settings >>>\n{official}")}),
        json!({"values": null}),
        json!({"values": []}),
        json!({"values": {"armstub": "other.fd"}}),
        json!({"values": {"enable_uart": false}}),
        json!({"values": {"enable_uart": "1\narmstub=other.fd"}}),
        json!({"values": {"enable_uart": "1\r"}}),
        json!({"values": {"enable_uart": ""}}),
        json!({"userText": 1}),
        json!({"userText": null}),
        json!({"previousDefaults": []}),
        json!({"previousDefaults": {}}),
        json!({"hdmiMode": "invalid"}),
        json!({"hdmiMode": false}),
        json!({"hdmiMode": null}),
        json!({"reset": "true"}),
        json!({"reset": 1}),
        json!({"hdmiChoices": null}),
        json!({"hdmiChoices": []}),
        json!({"hdmiChoices": [{"value": "bogus", "label": "invalid"}]}),
        json!({"hdmiChoices": [{"value": "custom", "label": 1}]}),
        json!({"hdmiChoices": ["custom"]}),
        json!({"config": format!("{official}\n[pi4]\nenable_uart=0\n"), "values": {"enable_uart": 1}}),
        json!({"config": format!("{official}\n[pi4]\nenable_uart=0\n"), "values": {"enable_uart": "0"}}),
        json!({"config": format!("{official}\n[pi4]\nenable_uart=0\n"), "reset": true}),
        json!({"config": format!("{official}\nhdmi_mode:0=16\n"), "hdmiMode": "official"}),
        json!({"config": format!("{official}\nhdmi_ignore_hotplug=1\n"), "hdmiMode": "official"}),
        json!({"recommended": ""}),
    ] {
        cases.push((format!("desktop boundary {changes}"), with(base, changes)));
    }
    let pi3 = &contexts()["3"];
    for changes in [
        json!({"values": {"core_freq": "300"}}),
        json!({"values": {"core_freq": ""}}),
        json!({"config": format!("{}\n[pi3]\ncore_freq=250\n[pi4]\ncore_freq=300\n", pi3["official"].as_str().unwrap())}),
        json!({"config": format!("{}\ncore_freq=0300\n", pi3["official"].as_str().unwrap()), "reset": true}),
    ] {
        cases.push((format!("Pi 3 clock {changes}"), with(pi3, changes)));
    }
    for request in [json!(null), json!([]), json!("object"), json!(1)] {
        cases.push(("non-object request".into(), request));
    }
    let iot = json!({
        "profile": "iot-core",
        "config": "hdmi_force_hotplug=1\nhdmi_group=1\nhdmi_mode=4\n"
    });
    for changes in [
        json!({}),
        json!({"config": ""}),
        json!({"reset": true}),
        json!({"hdmiMode": "official"}),
        json!({"hdmiMode": "720p60"}),
        json!({"hdmiMode": "1080p60"}),
        json!({"hdmiMode": "invalid"}),
        json!({"hdmiMode": null}),
        json!({"hdmiMode": 1}),
        json!({"values": null}),
        json!({"values": []}),
        json!({"values": {"hdmi_cvt": "1920 1080 60"}}),
        json!({"values": {"hdmi_drive": "2", "hdmi_mode": "16"}}),
        json!({"values": {"hdmi_drive": 2, "hdmi_group": 1, "hdmi_mode": 16}}),
        json!({"values": {"hdmi_force_hotplug": ""}}),
        json!({"values": {"hdmi_force_hotplug": "2"}}),
        json!({"values": {"hdmi_drive": "0"}}),
        json!({"values": {"hdmi_mode": true}}),
        json!({"values": {"hdmi_mode": null}}),
        json!({"values": {"hdmi_mode": [4]}}),
        json!({"values": {"hdmi_mode": "4\nhdmi_mode=16"}}),
        json!({"values": {"hdmi_mode": "4\narm_64bit=1"}}),
        json!({"values": {"hdmi_mode": "4 # café"}}),
        json!({"values": {"hdmi_mode": "4\u{0001}"}}),
        json!({"values": {"hdmi_mode": "4".repeat(513)}}),
        json!({"config": "hdmi_group=1\nhdmi_mode=65\n"}),
        json!({"config": "hdmi_group=2\nhdmi_mode=87\n"}),
        json!({"config": "hdmi_group=2\nhdmi_mode=86\n"}),
        json!({"config": "hdmi_group=0\nhdmi_mode=4\n"}),
        json!({"config": "hdmi_group=1\nhdmi_mode=0\n"}),
        json!({"config": "hdmi_group=1\nhdmi_mode=004\n"}),
        json!({"config": "hdmi_group=1\nhdmi_mode=0x4\n"}),
        json!({"config": "hdmi_group=1\nhdmi_mode=4\nhdmi_mode=16\n"}),
        json!({"config": "hdmi_group=1\nhdmi_mode=4\narm_64bit=1\n"}),
        json!({"config": "[pi3]\nhdmi_group=1\nhdmi_mode=4\n"}),
        json!({"config": "hdmi_group:0=1\nhdmi_mode=4\n"}),
        json!({"userText": "hdmi_group=2\nhdmi_mode=86\n"}),
        json!({"userText": "[pi3]\nhdmi_group=2\n[all]\nhdmi_group=1\nhdmi_mode=4\n"}),
        json!({"userText": "hdmi_group=1\nhdmi_mode=4\ngpu_mem=64\n"}),
        json!({"userText": null}),
    ] {
        cases.push((format!("IoT boundary {changes}"), with(&iot, changes)));
    }
    cases
}

#[test]
fn native_matches_python_normalization_and_validation_branches() {
    let cases = corpus();
    eprintln!(
        "comparing {} source-anchored boot-config fixtures",
        cases.len()
    );
    for (label, request) in cases {
        compare(&label, &serde_json::to_vec(&request).unwrap());
    }
}

#[test]
fn native_matches_python_across_transactional_saves_and_model_rebasing() {
    for model in ["3", "4", "5"] {
        let base = &contexts()[model];
        let request = with(
            base,
            json!({
                "config": format!("{}\n# Personal hardware\ncustom_option=keep\n",
                    base["recommended"].as_str().unwrap()),
                "values": {"disable_overscan": "0"}
            }),
        );
        let first = compare("initial save", &serde_json::to_vec(&request).unwrap());
        for changes in [
            json!({"config": first["config"]}),
            json!({"config": first["config"], "userText": first["userText"]}),
            json!({"config": first["config"], "reset": true}),
            json!({"config": first["config"], "hdmiMode": "official"}),
            json!({"config": first["config"], "userText": "# Replaced additions\ncustom_option=keep\n"}),
        ] {
            compare(
                "subsequent save",
                &serde_json::to_vec(&with(base, changes)).unwrap(),
            );
        }
        for next_model in ["3", "4", "5"] {
            compare(
                "model rebase",
                &serde_json::to_vec(&with(
                    &contexts()[next_model],
                    json!({"config": first["config"], "previousDefaults": base}),
                ))
                .unwrap(),
            );
        }
    }
}

#[test]
fn native_json_errors_and_input_limit_preserve_stdout_and_status() {
    let inputs: Vec<String> = serde_json::from_str(include_str!(
        "../../../tests/fixtures/boot-config-invalid-json.json"
    ))
    .unwrap();
    for input in inputs {
        compare("malformed JSON", input.as_bytes());
    }
    compare("input limit", " ".repeat(512 * 1024 + 1).as_bytes());
}

#[test]
fn native_engine_validation_and_model_navigation_use_the_cli() {
    let output = engine(
        r#"
RPI_MODEL=4 CONFIG_TXT='' CONFIG_TXT_MODEL='' APPLY_CUSTOM_CONFIG_TXT=1
set_default_config_txt || exit 91
CONFIG_TXT="$CONFIG_TXT
custom_option=keep"
validate_boot_config || exit 92
RPI_MODEL=3
set_default_config_txt || exit 93
validate_boot_config || exit 94
printf '%s\n' "$CONFIG_TXT"
"#,
        &[],
        true,
    );
    assert!(output.status.success(), "{output:?}");
    let config = String::from_utf8(output.stdout).unwrap();
    assert!(config.contains("device_tree_address=0x1f0000"));
    assert!(config.contains("custom_option=keep"));
    assert!(!config.contains("upstream-pi4"));
    let context = engine("boot_config_editor_context iot-core", &[], true);
    assert!(context.status.success(), "{context:?}");
    let context: Value = serde_json::from_slice(&context.stdout).unwrap();
    assert_eq!(context["executable"], binary());
    assert_eq!(context["arguments"], json!(["boot-config"]));
}

#[cfg(unix)]
#[test]
fn explicitly_selected_native_failures_never_retry_python() {
    use std::os::unix::fs::PermissionsExt;
    let parent = root().join("target/boot-config-tests");
    std::fs::create_dir_all(&parent).unwrap();
    let directory = tempfile::tempdir_in(parent).unwrap();
    let selected = directory.path().join("selected cli");
    std::fs::write(
        &selected,
        "#!/bin/bash\nprintf '%s\\n' '{\"ok\":false,\"error\":\"native sentinel\"}'\nexit 27\n",
    )
    .unwrap();
    std::fs::set_permissions(&selected, std::fs::Permissions::from_mode(0o700)).unwrap();
    let output = engine(
        "RPI_MODEL=4; boot_config_state \"$(config_txt_with_hdmi 4 official)\"",
        &[("WOR_RUST_BOOT_CONFIG", selected.to_str().unwrap())],
        false,
    );
    assert_eq!(output.status.code(), Some(27), "{output:?}");
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response, json!({"ok": false, "error": "native sentinel"}));
    for path in ["", "relative-cli", "/missing/wor-flasher"] {
        let output = engine(
            "RPI_MODEL=4; boot_config_state \"$(config_txt_with_hdmi 4 official)\"",
            &[("WOR_RUST_BOOT_CONFIG", path)],
            false,
        );
        assert!(!output.status.success(), "{path}: {output:?}");
    }
    let output = engine(
        r#"
RPI_MODEL=4 CONFIG_TXT="$(config_txt_with_hdmi 4 official)" APPLY_CUSTOM_CONFIG_TXT=1
original="$CONFIG_TXT"
if validate_boot_config;then exit 91;fi
[ "$CONFIG_TXT" == "$original" ] || exit 92
"#,
        &[("WOR_RUST_BOOT_CONFIG", selected.to_str().unwrap())],
        false,
    );
    assert!(output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("native sentinel"));
}

#[test]
fn classic_engine_selection_remains_python() {
    let output = engine("RPI_MODEL=4; boot_config_editor_context", &[], false);
    assert!(output.status.success(), "{output:?}");
    let context: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(context.get("executable").is_none());
    assert!(
        context["helper"]
            .as_str()
            .unwrap()
            .ends_with("boot-config.py")
    );
    assert!(!context["python"].as_str().unwrap().is_empty());
}

#[test]
fn jxa_editor_uses_one_selected_process_and_propagates_native_failures() {
    let mut command = Command::new("node");
    command.arg("--input-type=module").arg("-e").arg(
        r#"
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";
const source = readFileSync("src/lib/boot-config-gui.js", "utf8");
for (const native of [true, false]) {
  for (const failure of [true, false]) {
    let launches = 0;
    const response = JSON.stringify(failure ? { ok: false, error: "native sentinel" } : { ok: true, config: "safe" });
    const task = { terminationStatus: failure ? 27 : 0, get launch() { launches++; } };
    const $ = (value) => ({ value, dataUsingEncoding() { return value; } });
    $.NSTask = { alloc: { init: task } };
    $.NSPipe = { get pipe() { return {
      fileHandleForWriting: { writeData() {}, get closeFile() {} },
      fileHandleForReading: { get readDataToEndOfFile() { return response; } }
    }; } };
    $.NSString = { alloc: { initWithDataEncoding(value) { return value; } } };
    const context = vm.createContext({ $, ObjC: { unwrap: (value) => value } });
    vm.runInContext(source, context);
    const settings = native
      ? { executable: "/verified cli", arguments: ["boot-config"] }
      : { python: "/python3", helper: "/boot-config.py" };
    if (failure) assert.throws(() => context.worBootConfigCall(settings, {}), /native sentinel/);
    else assert.equal(context.worBootConfigCall(settings, {}).config, "safe");
    assert.equal(launches, 1);
    assert.equal(task.launchPath.value, native ? "/verified cli" : "/python3");
    assert.equal(JSON.stringify(task.arguments.value), JSON.stringify(native ? ["boot-config"] : ["/boot-config.py"]));
  }
}
"#,
    );
    let output = invoke(command, b"");
    assert!(output.status.success(), "{output:?}");
}
