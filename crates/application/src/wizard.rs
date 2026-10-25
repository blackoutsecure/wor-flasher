use serde_json::{Value, json};
use wor_domain::configuration::{NativeConfig, at_path};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Image,
    Drive,
    Options,
    Review,
    Progress,
    Result,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FormField {
    pub path: String,
    pub description: String,
    pub kind: String,
    pub options: Vec<Value>,
    pub secret: bool,
    pub nullable: bool,
}

pub fn schema_fields(schema: &Value) -> Vec<FormField> {
    fn walk(schema: &Value, prefix: &str, out: &mut Vec<FormField>) {
        if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
            for (key, child) in properties {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                if child.get("properties").is_some() {
                    walk(child, &path, out);
                } else {
                    let types = child.get("type");
                    let kind = types
                        .and_then(Value::as_str)
                        .or_else(|| {
                            types.and_then(Value::as_array).and_then(|types| {
                                types
                                    .iter()
                                    .filter_map(Value::as_str)
                                    .find(|value| *value != "null")
                            })
                        })
                        .unwrap_or("string");
                    out.push(FormField {
                        secret: key.to_ascii_lowercase().contains("password"),
                        description: child
                            .get("description")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .into(),
                        options: child
                            .get("enum")
                            .and_then(Value::as_array)
                            .cloned()
                            .unwrap_or_default(),
                        nullable: types
                            .and_then(Value::as_array)
                            .is_some_and(|types| types.iter().any(|value| value == "null")),
                        kind: kind.into(),
                        path,
                    });
                }
            }
        }
    }
    let mut result = Vec::new();
    walk(schema, "", &mut result);
    result
}

pub fn set_value(document: &mut Value, path: &str, value: Option<Value>) {
    let parts: Vec<_> = path.split('.').collect();
    let mut current = document;
    for part in parts.iter().take(parts.len().saturating_sub(1)) {
        if !current.get(*part).is_some_and(Value::is_object) {
            current[*part] = json!({});
        }
        current = &mut current[*part];
    }
    if let Some(last) = parts.last() {
        match value {
            Some(value) => current[*last] = value,
            None => {
                if let Some(object) = current.as_object_mut() {
                    object.remove(*last);
                }
            }
        }
    }
}

pub fn switch_family(document: &mut Value, iot: bool) {
    set_value(
        document,
        "media.imageFamily",
        Some(json!(if iot { "iot-core" } else { "desktop" })),
    );
    set_value(
        document,
        "media.architecture",
        Some(json!(if iot { "arm32" } else { "arm64" })),
    );
    set_value(document, "media.sourceFile", Some(json!("")));
    set_value(document, "media.downloadIotImage", Some(json!(iot)));
    set_value(document, "execution.confirmIotErase", Some(json!(false)));
    set_value(document, "execution.confirmIotWipe", Some(json!(false)));
    if iot {
        let board = at_path(document, "target.board")
            .and_then(Value::as_str)
            .unwrap_or("");
        if !matches!(board, "pi2-v1.1" | "pi2-v1.2" | "pi3-b") {
            set_value(document, "target.board", Some(json!("pi3-b")));
        }
        set_value(document, "target.canInstallOnSameDrive", Some(json!(true)));
        set_value(
            document,
            "execution.skipImageVerification",
            Some(json!(false)),
        );
        set_value(document, "userAccount.setupAccount", Some(json!(false)));
        set_value(document, "userAccount.password", Some(json!("")));
    } else if at_path(document, "target.board") == Some(&json!("pi2-v1.1")) {
        set_value(document, "target.board", None);
        set_value(document, "target.rpiModel", Some(json!(4)));
    }
}

pub fn applicable(field: &FormField, config: &NativeConfig, macos: bool) -> bool {
    let path = field.path.as_str();
    let iot = config.get("WOR_IMAGE_FAMILY") == "iot-core";
    if path == "$schema"
        || path == "media.architecture"
        || path == "execution.confirmIotErase"
        || path == "execution.confirmIotWipe"
    {
        return false;
    }
    if path.starts_with("notifications.sounds.") {
        return path
            == if macos {
                "notifications.sounds.macos"
            } else {
                "notifications.sounds.linux"
            };
    }
    if path.starts_with("userAccount.iotCore.")
        || path.starts_with("customization.iot")
        || path == "media.downloadIotImage"
        || path == "execution.wipeIotDrive"
    {
        if !iot {
            return false;
        }
        if matches!(
            path,
            "userAccount.iotCore.username" | "userAccount.iotCore.password"
        ) {
            return config.get("IOT_CORE_ACCOUNT_SETUP") == "1";
        }
        return true;
    }
    if iot {
        return !path.starts_with("customization.")
            && !path.starts_with("system.")
            && !path.starts_with("userAccount.")
            && !matches!(
                path,
                "target.rpiModel"
                    | "target.canInstallOnSameDrive"
                    | "media.winLang"
                    | "media.bid"
                    | "media.useCache"
                    | "execution.skipImageVerification"
                    | "execution.verifyTls"
            );
    }
    if path == "target.board" {
        return false;
    }
    if path == "customization.oobeNetworkBypass" {
        return config
            .windows_label()
            .is_ok_and(|value| value == "Windows 11");
    }
    if matches!(
        path,
        "customization.pi4AutoDisable3Gb" | "customization.pi4UefiShellUnlock"
    ) {
        return config.get("RPI_MODEL") == "4";
    }
    if matches!(path, "customization.driversUseLatest" | "system.driverVer") {
        return config.get("RPI_MODEL") != "5";
    }
    if path.starts_with("system.uefiVerPi") {
        return path.ends_with(config.get("RPI_MODEL"));
    }
    if matches!(path, "userAccount.username" | "userAccount.password") {
        return config.get("WINDOWS_ACCOUNT_SETUP") == "1";
    }
    true
}

pub fn title(path: &str) -> String {
    let name = path.rsplit('.').next().unwrap_or(path);
    let mut out = String::new();
    for character in name.chars() {
        if character.is_ascii_uppercase() && !out.is_empty() {
            out.push(' ');
        }
        out.push(character);
    }
    if let Some(first) = out.get_mut(..1) {
        first.make_ascii_uppercase();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    fn config(document: &Value) -> NativeConfig {
        NativeConfig::resolve(
            document,
            &BTreeMap::new(),
            &serde_json::from_str(include_str!("../../../src/config/metadata.json"))
                .expect("metadata"),
            "/tmp",
            "en_US",
            true,
        )
        .expect("config")
    }
    #[test]
    fn every_schema_leaf_is_exposed_without_raw_credential_json_editing() {
        let schema: Value =
            serde_json::from_str(include_str!("../../../config-templates/config.schema.json"))
                .expect("schema");
        let fields = schema_fields(&schema);
        assert!(fields.len() > 60);
        for path in [
            "userAccount.password",
            "userAccount.iotCore.password",
            "userAccount.iotCore.currentPassword",
        ] {
            assert!(
                fields
                    .iter()
                    .any(|field| field.path == path && field.secret)
            );
        }
        for path in [
            "customization.pi4UefiShellUnlock",
            "execution.wipeIotDrive",
            "customization.iotHdmiConfig",
            "notifications.sounds.macos",
        ] {
            assert!(fields.iter().any(|field| field.path == path));
        }
    }
    #[test]
    fn family_switch_drops_cross_profile_sources_and_requires_fresh_erase_consent() {
        let mut document = json!({"target":{"rpiModel":4},"media":{"sourceFile":"desktop.iso"},"execution":{"confirmIotErase":true,"skipImageVerification":true},
            "userAccount":{"setupAccount":true,"password":"fixture-secret"}});
        switch_family(&mut document, true);
        assert_eq!(at_path(&document, "media.sourceFile"), Some(&json!("")));
        assert_eq!(
            at_path(&document, "media.architecture"),
            Some(&json!("arm32"))
        );
        assert_eq!(at_path(&document, "target.board"), Some(&json!("pi3-b")));
        assert_eq!(
            at_path(&document, "execution.confirmIotErase"),
            Some(&json!(false))
        );
        assert_eq!(
            at_path(&document, "execution.skipImageVerification"),
            Some(&json!(false))
        );
        assert_eq!(at_path(&document, "userAccount.password"), Some(&json!("")));
    }
    #[test]
    fn contextual_options_share_domain_model_rules() {
        let schema: Value =
            serde_json::from_str(include_str!("../../../config-templates/config.schema.json"))
                .expect("schema");
        let fields = schema_fields(&schema);
        let field = |path: &str| {
            fields
                .iter()
                .find(|field| field.path == path)
                .expect("field")
        };
        let desktop = config(&json!({"target":{"rpiModel":5},"media":{"bid":"26200.1"}}));
        assert!(!applicable(
            field("customization.pi4AutoDisable3Gb"),
            &desktop,
            true
        ));
        assert!(!applicable(
            field("customization.driversUseLatest"),
            &desktop,
            true
        ));
        assert!(applicable(
            field("customization.oobeNetworkBypass"),
            &desktop,
            true
        ));
        let iot = config(&json!({"target":{"board":"pi3-b"},"media":{"imageFamily":"iot-core"}}));
        assert!(!applicable(
            field("execution.skipImageVerification"),
            &iot,
            true
        ));
        assert!(applicable(
            field("userAccount.iotCore.currentPassword"),
            &iot,
            true
        ));
        assert!(!applicable(
            field("notifications.sounds.windows"),
            &iot,
            true
        ));
    }
}
