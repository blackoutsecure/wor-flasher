use serde_json::{Value, json};
use std::collections::BTreeMap;
use wor_domain::configuration::{DriveCapability, ImageFamily, NativeConfig};

fn resolve(document: Value, entries: &[(&str, &str)]) -> NativeConfig {
    let environment = entries
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
    NativeConfig::resolve(
        &document,
        &environment,
        &serde_json::from_str(include_str!("../../../src/config/metadata.json")).expect("metadata"),
        "/tmp/wor-native-home",
        "en_GB.UTF-8",
        true,
    )
    .expect("valid configuration")
}

#[test]
fn nested_false_and_environment_precedence_match_the_source_loader() {
    let config = resolve(
        json!({
            "notifications":{"playSound":false},
            "PLAY_SOUND":true,
            "execution":{"dryRun":false},
            "DRY_RUN":true,
            "customization":{"uefiUseLatest":false}
        }),
        &[("DRY_RUN", "1")],
    );
    assert!(!config.flag("PLAY_SOUND").expect("boolean"));
    assert!(config.flag("DRY_RUN").expect("boolean"));
    assert!(!config.flag("UEFI_USE_LATEST").expect("explicit boolean"));
}

#[test]
fn explicit_empty_current_credentials_are_not_replaced_by_json() {
    let config = resolve(
        json!({"userAccount":{"iotCore":{
            "currentUsername":"configured-user","currentPassword":"fixture-secret"
        }}}),
        &[
            ("IOT_CORE_CURRENT_USERNAME", ""),
            ("IOT_CORE_CURRENT_PASSWORD", ""),
        ],
    );
    assert_eq!(config.get("IOT_CORE_CURRENT_USERNAME"), "");
    assert_eq!(config.get("IOT_CORE_CURRENT_PASSWORD"), "");
    assert!(config.is_set("IOT_CORE_CURRENT_PASSWORD"));
}

#[test]
fn malformed_boolean_preferences_fail_without_echoing_sensitive_values() {
    let error = NativeConfig::resolve(
        &json!({"execution":{"dryRun":"fixture-sensitive-invalid"}}),
        &BTreeMap::new(),
        &serde_json::from_str(include_str!("../../../src/config/metadata.json")).expect("metadata"),
        "/tmp",
        "en-US",
        false,
    )
    .err()
    .expect("invalid boolean");
    assert!(error.to_string().contains("DRY_RUN"));
    assert!(!error.to_string().contains("fixture-sensitive-invalid"));
}

#[test]
fn metadata_and_model_owned_defaults_are_not_replaced_by_ui_defaults() {
    let config = resolve(json!({"target":{"rpiModel":3}}), &[]);
    assert_eq!(config.get("UEFI_VER_PI4"), "v1.50");
    assert_eq!(config.get("UEFI_VER_PI3"), "v1.39");
    assert!(config.effective_latest_uefi().expect("model default"));
    assert_eq!(
        config.get("DL_DIR"),
        "/tmp/wor-native-home/wor-flasher-files"
    );
    assert_eq!(config.get("WINDOWS_LOCALE"), "en-GB");
    let config = resolve(json!({"target":{"rpiModel":4}}), &[]);
    assert!(!config.effective_latest_uefi().expect("model default"));
}

#[test]
fn build_and_capacity_boundaries_are_exact() {
    for model in [3, 4] {
        for build in ["25162.1", "25163.0"] {
            resolve(
                json!({"target":{"rpiModel":model},"media":{"bid":build}}),
                &[],
            )
            .validate_profile()
            .expect("compatible build");
        }
        assert!(
            resolve(
                json!({"target":{"rpiModel":model},"media":{"bid":"25164.0"}}),
                &[]
            )
            .validate_profile()
            .is_err()
        );
    }
    resolve(
        json!({"target":{"rpiModel":5},"media":{"bid":"26200.1"}}),
        &[],
    )
    .validate_profile()
    .expect("Pi 5 build");
    let min = 8_u64 * 1024 * 1024 * 1024;
    let direct = 25_u64 * 1024 * 1024 * 1024;
    assert_eq!(DriveCapability::desktop(min - 1), DriveCapability::TooSmall);
    assert_eq!(DriveCapability::desktop(min), DriveCapability::Recovery);
    assert_eq!(
        DriveCapability::desktop(direct - 1),
        DriveCapability::Recovery
    );
    assert_eq!(DriveCapability::desktop(direct), DriveCapability::Install);
    assert_eq!(
        DriveCapability::desktop(direct + 1),
        DriveCapability::Install
    );
}

#[test]
fn profile_and_physical_board_never_collapse_into_the_package_route() {
    let config = resolve(json!({"target":{"rpiModel":3,"board":"pi2-v1.2"}}), &[]);
    assert_eq!(config.board_label(), "Raspberry Pi 2 v1.2");
    assert_eq!(config.model().expect("model"), 3);
    assert!(
        resolve(json!({"target":{"rpiModel":3,"board":"pi2-v1.1"}}), &[])
            .validate_profile()
            .is_err()
    );
    let config = resolve(
        json!({"media":{"imageFamily":"iot-core","architecture":"arm32"},
        "target":{"board":"pi3-b","canInstallOnSameDrive":true}}),
        &[],
    );
    assert_eq!(config.family().expect("family"), ImageFamily::IotCore);
    config.validate_profile().expect("supported IoT profile");
    for board in ["pi3-b-plus", "pi4", "pi5"] {
        assert!(
            resolve(
                json!({"media":{"imageFamily":"iot-core"},"target":{"board":board}}),
                &[]
            )
            .validate_profile()
            .is_err()
        );
    }
    assert!(
        resolve(
            json!({"media":{"imageFamily":"iot-core","architecture":"arm64"},
        "target":{"board":"pi3-b"}}),
            &[]
        )
        .validate_profile()
        .is_err()
    );
}

#[test]
fn iot_cannot_inherit_desktop_verification_or_recovery_bypasses() {
    for overrides in [
        json!({"target":{"board":"pi3-b","canInstallOnSameDrive":false},"media":{"imageFamily":"iot-core"}}),
        json!({"target":{"board":"pi3-b"},"media":{"imageFamily":"iot-core"},"execution":{"skipImageVerification":true}}),
        json!({"target":{"board":"pi3-b"},"media":{"imageFamily":"iot-core"},"userAccount":{"setupAccount":true,"username":"fixture","password":"fixture-secret"}}),
    ] {
        assert!(resolve(overrides, &[]).validate_profile().is_err());
    }
}

#[test]
fn source_compatible_aliases_update_precedence_and_home_paths_are_preserved() {
    let config = resolve(
        json!({"rpiModel":5,"checkForUpdates":true,"sourceFile":"~/images/local.iso",
        "execution":{"checkForUpdates":false},"userAccount":{"localeSetup":false}}),
        &[],
    );
    assert_eq!(config.model().expect("model"), 5);
    assert!(config.flag("NO_UPDATE").expect("disabled update check"));
    assert!(!config.flag("WINDOWS_LOCALE_SETUP").expect("false retained"));
    assert_eq!(
        config.get("SOURCE_FILE"),
        "/tmp/wor-native-home/images/local.iso"
    );
}

#[test]
fn update_inverse_and_platform_sound_keep_the_source_precedence() {
    let document = json!({"execution":{"checkForUpdates":true,"noUpdate":true},
        "completionSound":"legacy-sound","notifications":{"sounds":{"macos":"Glass","linux":"complete"}}});
    let config = resolve(document.clone(), &[]);
    assert!(
        !config
            .flag("NO_UPDATE")
            .expect("checkForUpdates wins over JSON noUpdate")
    );
    assert_eq!(config.get("COMPLETION_SOUND"), "Glass");
    let config = resolve(
        document,
        &[("NO_UPDATE", "1"), ("COMPLETION_SOUND", "explicit-sound")],
    );
    assert!(config.flag("NO_UPDATE").expect("explicit environment wins"));
    assert_eq!(config.get("COMPLETION_SOUND"), "explicit-sound");
    for (value, disabled) in [("true", false), ("1", false), ("false", true), ("0", true)] {
        assert_eq!(
            resolve(json!({}), &[("CHECK_FOR_UPDATES", value)])
                .flag("NO_UPDATE")
                .expect("source update preference"),
            disabled
        );
    }
}
