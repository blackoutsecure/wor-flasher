use crate::files::{
    copy_file, make_directory, read_required, required_file, sha256_file, write_private,
};
use crate::options::validate_digest;
use crate::{BuildId, DesktopOptions, MediaError, Model, check_cancelled};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

pub(crate) fn template(options: &DesktopOptions, name: &str) -> Result<String, MediaError> {
    let bytes = read_required(
        &options.runtime_dir.join("config-templates").join(name),
        "configuration template",
        1024 * 1024,
    )?;
    String::from_utf8(bytes)
        .map_err(|_| MediaError::InvalidMetadata("configuration template encoding"))
}

fn escape(text: &str) -> String {
    let mut result = String::new();
    for ch in text.chars() {
        result.push_str(match ch {
            '&' => "&amp;",
            '<' => "&lt;",
            '>' => "&gt;",
            '"' => "&quot;",
            '\'' => "&apos;",
            _ => {
                result.push(ch);
                continue;
            }
        });
    }
    result
}

/// Source-equivalent answer-file composition. Account credentials are included
/// only for an explicitly enabled account, and never returned in an error.
pub fn answer_file(
    options: &DesktopOptions,
    build: &BuildId,
) -> Result<Option<String>, MediaError> {
    let ram = options.model == Model::Pi4 && options.setup.pi4_auto_disable_3gb;
    let oobe =
        build.major() >= options.defaults.win11_min_build && options.setup.oobe_network_bypass;
    if !ram && !oobe && options.setup.account.is_none() && options.setup.locale.is_none() {
        return Ok(None);
    }
    let mut answer = String::from(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<unattend xmlns=\"urn:schemas-microsoft-com:unattend\" xmlns:wcm=\"http://schemas.microsoft.com/WMIConfig/2002/State\">\n",
    );
    if ram {
        answer.push_str(&template(options, "pi4-ram-unlock-specialize.xml")?);
    }
    if oobe {
        answer.push_str(&template(options, "oobe-network-bypass.xml")?);
    }
    if let Some((username, password)) = &options.setup.account {
        let username = escape(username);
        let password = escape(&password.0);
        answer.push_str(&format!("  <settings pass=\"oobeSystem\">\n    <component name=\"Microsoft-Windows-Shell-Setup\" processorArchitecture=\"arm64\" publicKeyToken=\"31bf3856ad364e35\" language=\"neutral\" versionScope=\"nonSxS\">\n      <UserAccounts><LocalAccounts><LocalAccount wcm:action=\"add\"><Password><Value>{password}</Value><PlainText>true</PlainText></Password><Description>WoR-Flasher local administrator</Description><DisplayName>{username}</DisplayName><Group>Administrators</Group><Name>{username}</Name></LocalAccount></LocalAccounts></UserAccounts>\n    </component>\n  </settings>\n"));
    }
    if let Some(locale) = &options.setup.locale {
        if !crate::options::valid_locale(locale) {
            return Err(MediaError::InvalidMetadata("Windows locale"));
        }
        answer.push_str(&format!("  <settings pass=\"oobeSystem\">\n    <component name=\"Microsoft-Windows-International-Core\" processorArchitecture=\"arm64\" publicKeyToken=\"31bf3856ad364e35\" language=\"neutral\" versionScope=\"nonSxS\"><InputLocale>{locale}</InputLocale><SystemLocale>{locale}</SystemLocale><UILanguage>{locale}</UILanguage><UILanguageFallback>en-US</UILanguageFallback><UserLocale>{locale}</UserLocale></component>\n  </settings>\n"));
    }
    answer.push_str("</unattend>\n");
    crate::xml::parse(answer.as_bytes())?;
    Ok(Some(answer))
}

pub(crate) fn boot_config(
    options: &DesktopOptions,
) -> Result<(Option<String>, Vec<String>), MediaError> {
    if !options.boot.apply_custom_config {
        return Ok((None, Vec::new()));
    }
    let official = format!(
        "\n\n{}",
        template(options, &format!("pi{}.config.txt", options.model.number()))?
            .trim_end_matches('\n')
    );
    let recommended = if options.model == Model::Pi4 {
        format!(
            "{official}\nhdmi_drive=2\nhdmi_group=2\nhdmi_mode=87\nhdmi_cvt=1920 1200 60 6 0 0 0"
        )
    } else {
        official.clone()
    };
    let config = options.boot.config_txt.as_deref().unwrap_or_else(|| {
        if options.boot.hdmi_mode == "recommended" {
            &recommended
        } else {
            &official
        }
    });
    let mut values = options.boot.settings.clone();
    for value in values
        .as_object_mut()
        .ok_or(MediaError::InvalidMetadata("boot settings object"))?
        .values_mut()
    {
        *value = match value {
            serde_json::Value::Bool(boolean) => json!(if *boolean { "1" } else { "0" }),
            serde_json::Value::Null => json!(""),
            serde_json::Value::Number(number) => json!(number.to_string()),
            serde_json::Value::String(_) => value.clone(),
            _ => return Err(MediaError::InvalidMetadata("boot settings value")),
        };
    }
    let mut choices = vec![json!({"value":"official","label":"Official image default"})];
    if options.model == Model::Pi4 {
        choices.push(json!({"value":"recommended","label":"1920 x 1200 / 60 Hz (recommended)"}));
    }
    choices.push(json!({"value":"custom","label":"Custom video settings"}));
    let request = json!({
        "profile":"desktop", "model":options.model.number().to_string(),
        "official":official, "recommended":recommended, "hdmiChoices":choices,
        "config":config, "values":values,
    });
    let result = wor_domain::boot_config::process(&request).map_err(|_| {
        crate::options::option(
            "CONFIG_TXT",
            "boot configuration is invalid or changes protected firmware settings",
        )
    })?;
    let config = result["config"]
        .as_str()
        .ok_or(MediaError::InvalidMetadata("normalized boot configuration"))?
        .to_owned();
    let warnings = result["warnings"]
        .as_array()
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    Ok((Some(config), warnings))
}

pub(crate) fn validate_pi3_helper(
    options: &DesktopOptions,
    cancelled: &AtomicBool,
) -> Result<(), MediaError> {
    if options.model != Model::Pi3 {
        return Ok(());
    }
    let directory = options.runtime_dir.join("src/lib/pi3-boot-refresh");
    let manifest: serde_json::Value = serde_json::from_slice(&read_required(
        &directory.join("manifest.json"),
        "qualified Pi 3 finalizer manifest",
        128 * 1024,
    )?)
    .map_err(|_| MediaError::InvalidMetadata("qualified Pi 3 finalizer manifest"))?;
    if manifest["target"] != "windows/arm64" {
        return Err(MediaError::InvalidMetadata(
            "qualified Pi 3 finalizer architecture",
        ));
    }
    for (name, key) in [
        ("Pi3BootRefresh.exe", "binarySha256"),
        ("GO-LICENSE.txt", "goLicenseSha256"),
    ] {
        let digest = manifest[key]
            .as_str()
            .ok_or(MediaError::InvalidMetadata("Pi 3 finalizer digest"))?;
        validate_digest(digest, 64, "Pi 3 finalizer digest")?;
        let path = directory.join(name);
        required_file(&path, "qualified Pi 3 finalizer")?;
        if !sha256_file(&path, cancelled)?.eq_ignore_ascii_case(digest) {
            return Err(MediaError::Integrity {
                component: "qualified Pi 3 finalizer",
            });
        }
    }
    Ok(())
}

pub(crate) fn configure_pe(
    options: &DesktopOptions,
    pe: &Path,
    answer: Option<&str>,
    shell: Option<&Path>,
    cancelled: &AtomicBool,
) -> Result<PathBuf, MediaError> {
    check_cancelled(cancelled)?;
    let app = pe.join("winpe/2");
    let settings_path = app.join("settings.ini");
    let bytes = read_required(&settings_path, "WoR-PE settings.ini", 1024 * 1024)?;
    let text = String::from_utf8(bytes)
        .map_err(|_| MediaError::InvalidMetadata("WoR-PE settings encoding"))?;
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut found = false;
    let mut settings = String::new();
    for line in text.lines() {
        if line.starts_with("HideEmptyDrives=") {
            settings.push_str(&format!(
                "HideEmptyDrives={}\n",
                u8::from(options.boot.hide_empty_drives)
            ));
            found = true;
        } else {
            settings.push_str(line);
            settings.push('\n');
        }
    }
    if !found {
        settings.push_str(&format!(
            "HideEmptyDrives={}\n",
            u8::from(options.boot.hide_empty_drives)
        ));
    }
    write_private(&settings_path, settings.as_bytes())?;
    let scripts = app.join("scripts");
    if scripts.exists() {
        crate::files::require_directory(&scripts, "private PE scripts")?;
        fs::remove_dir_all(&scripts)
            .map_err(|e| MediaError::io("discard previous PE customization", e))?;
    }
    validate_pi3_helper(options, cancelled)?;
    let pi3 = options.model == Model::Pi3;
    if answer.is_none() && shell.is_none() && !pi3 {
        return Ok(app);
    }
    make_directory(&scripts)?;
    if pi3 {
        for name in ["Pi3BootRefresh.exe", "GO-LICENSE.txt"] {
            copy_file(
                &options
                    .runtime_dir
                    .join("src/lib/pi3-boot-refresh")
                    .join(name),
                &scripts.join(name),
                cancelled,
            )?;
        }
    }
    if let Some(answer) = answer {
        write_private(&scripts.join("unattend.xml"), answer.as_bytes())?;
    }
    if options.model == Model::Pi4 && options.setup.pi4_auto_disable_3gb {
        write_private(
            &scripts.join("Pi4Disable3GB.ps1"),
            template(options, "pi4-ram-unlock.ps1")?.as_bytes(),
        )?;
    }
    if let Some(shell) = shell {
        copy_file(shell, &scripts.join("Shell.efi"), cancelled)?;
    }
    let batch = template(options, "prefinalize.cmd")?
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\n', "\r\n");
    write_private(&scripts.join("prefinalize.cmd"), batch.as_bytes())?;
    Ok(app)
}

pub(crate) fn stage_answers(
    options: &DesktopOptions,
    answer: Option<&str>,
    boot: &Path,
    install: &Path,
) -> Result<(), MediaError> {
    if let Some(answer) = answer {
        for destination in [boot, install] {
            write_private(&destination.join("Autounattend.xml"), answer.as_bytes())?;
            if options.model == Model::Pi4 && options.setup.pi4_auto_disable_3gb {
                write_private(
                    &destination.join("Pi4Disable3GB.ps1"),
                    template(options, "pi4-ram-unlock.ps1")?.as_bytes(),
                )?;
            }
        }
    }
    Ok(())
}
