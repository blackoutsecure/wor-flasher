use serde_json::{Value, json};
use wor_domain::boot_config::{ConfigError, process, process_json};

fn desktop() -> Value {
    let official = include_str!("../../../config-templates/pi4.config.txt");
    json!({
        "model": "4", "official": official, "recommended": official, "config": official,
        "hdmiChoices": [
            {"value": "official", "label": "Official image default"},
            {"value": "custom", "label": "Custom video settings"}
        ]
    })
}

fn rejected(request: &Value, expected: &str) {
    let error: ConfigError = process(request).expect_err("a boot safeguard accepted invalid input");
    assert!(error.to_string().contains(expected), "{error}");
}

#[test]
fn native_rejects_guard_removal_regressions() {
    let mut request = desktop();
    let official = request["official"].as_str().unwrap().to_owned();
    request["config"] = json!(format!("{official}\nenable_uart=2\n"));
    rejected(&request, "enable_uart must be 0");
    request["config"] = json!(format!("{official}\n[pi4]\narmstub=other.fd\n"));
    rejected(&request, "armstub is required");
    request["config"] = json!(official.replace("dtoverlay=upstream-pi4", ""));
    rejected(&request, "Required boot overlay");
    rejected(
        &json!({"profile": "iot-core", "config": "hdmi_group=1\nhdmi_mode=65\n"}),
        "legacy Pi 2/3",
    );
}

#[test]
fn structured_values_do_not_bypass_protected_or_conditional_settings() {
    let mut request = desktop();
    request["values"] = json!({"armstub": "other.fd"});
    rejected(&request, "unsupported control");
    request["values"] = json!({"enable_uart": true});
    rejected(&request, "Invalid value");
    request["values"] = json!({"enable_uart": "0"});
    request["config"] = json!(format!(
        "{}\n[pi4]\nenable_uart=0\n[all]\n",
        request["official"].as_str().unwrap()
    ));
    rejected(&request, "advanced conditional settings");
}

#[test]
fn normalization_is_loss_aware_and_idempotent() {
    let mut request = desktop();
    request["config"] = json!(format!(
        "{}\nenable_uart=0 # earlier\n\
         enable_uart=1 # later\n\
         # Personal hardware\n\
         dtparam=i2c_arm=on\ndtparam=audio=on\n\
         [HDMI:0]\nhdmi_drive=1\n[all]\nhdmi_drive=2\n",
        request["official"].as_str().unwrap()
    ));
    let first = process(&request).unwrap();
    assert!(first["userText"].as_str().unwrap().contains("# earlier"));
    assert!(first["userText"].as_str().unwrap().contains("# later"));
    assert!(
        first["config"]
            .as_str()
            .unwrap()
            .contains("[HDMI:0]\nhdmi_drive=1\n[all]\nhdmi_drive=2")
    );
    request["config"] = first["config"].clone();
    let second = process(&request).unwrap();
    assert_eq!(second["config"], first["config"]);
    assert_eq!(second["userText"], first["userText"]);
}

#[test]
fn integers_preserve_arbitrary_precision_and_equivalent_required_values() {
    let mut request = desktop();
    request["config"] = json!(
        request["official"]
            .as_str()
            .unwrap()
            .replace("arm_64bit=1", "arm_64bit=0x1")
            .replace(
                "device_tree_address=0x3e0000",
                "device_tree_address=4063232"
            )
            .to_owned()
            + "\nboot_delay=9999999999999999999999999999999999999999999999999999\n"
    );
    let state = process(&request).unwrap();
    assert!(
        state["config"]
            .as_str()
            .unwrap()
            .contains("boot_delay=9999999999999999999999999999999999999999999999999999")
    );
}

#[test]
fn iot_is_display_only_even_when_using_advanced_text() {
    rejected(
        &json!({"profile": "iot-core", "config": "hdmi_group=1\nhdmi_mode=4\narm_64bit=1\n"}),
        "Boot entries stay protected",
    );
    rejected(
        &json!({
            "profile": "iot-core", "config": "hdmi_group=1\nhdmi_mode=4\n",
            "userText": "hdmi_group=1\nhdmi_mode=4\ngpu_mem=64\n"
        }),
        "HDMI-only",
    );
}

#[test]
fn malformed_json_reports_precise_character_positions() {
    for (input, expected) in [
        ("[1", "Expecting ',' delimiter: line 1 column 3 (char 2)"),
        (
            "{\"model\"",
            "Expecting ':' delimiter: line 1 column 9 (char 8)",
        ),
        (
            "{\"model\": 4,",
            "Expecting property name enclosed in double quotes: line 1 column 13 (char 12)",
        ),
        ("tru", "Expecting value: line 1 column 1 (char 0)"),
        ("01", "Extra data: line 1 column 2 (char 1)"),
        ("1e+", "Extra data: line 1 column 2 (char 1)"),
        (
            "\"a\\\"unterminated",
            "Unterminated string starting at: line 1 column 1 (char 0)",
        ),
        (
            "{\"model\": \"\\uZZZZ\"}",
            "Invalid \\uXXXX escape: line 1 column 13 (char 12)",
        ),
        ("{\"é\": nope}", "Expecting value: line 1 column 7 (char 6)"),
    ] {
        assert_eq!(process_json(input).unwrap_err().to_string(), expected);
    }
}
