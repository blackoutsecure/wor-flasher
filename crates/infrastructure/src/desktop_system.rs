use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;
use thiserror::Error;
use wor_domain::configuration::NativeConfig;

#[derive(Debug, Error)]
pub enum DesktopSystemError {
    #[error("Desktop integration failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Failed(String),
}

fn locale_from_sources(
    macos: bool,
    apple_locale: Option<&str>,
    environment: [Option<&str>; 3],
) -> String {
    let locale = if macos {
        apple_locale
    } else {
        environment
            .into_iter()
            .flatten()
            .find(|value| !value.trim().is_empty())
    };
    locale
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("en_US.UTF-8")
        .to_owned()
}

pub fn host_locale() -> String {
    let apple_locale = if cfg!(target_os = "macos") {
        match Command::new("/usr/bin/defaults")
            .args(["read", "-g", "AppleLocale"])
            .output()
        {
            Ok(output) if output.status.success() => match String::from_utf8(output.stdout) {
                Ok(value) if !value.trim().is_empty() => Some(value),
                _ => {
                    tracing::warn!("macOS regional preference is invalid; using en-US");
                    None
                }
            },
            Ok(_) => {
                tracing::warn!("macOS regional preference is unavailable; using en-US");
                None
            }
            Err(error) => {
                tracing::warn!(error = %error, "Cannot read macOS regional preference; using en-US");
                None
            }
        }
    } else {
        None
    };
    locale_from_sources(
        cfg!(target_os = "macos"),
        apple_locale.as_deref(),
        [
            std::env::var("LC_ALL").ok().as_deref(),
            std::env::var("LC_MESSAGES").ok().as_deref(),
            std::env::var("LANG").ok().as_deref(),
        ],
    )
}

pub fn open_privacy_settings(full_disk: bool) -> Result<(), DesktopSystemError> {
    let uri = if full_disk {
        "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles"
    } else {
        "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
    };
    if !cfg!(target_os = "macos") {
        return Err(DesktopSystemError::Failed(
            "This settings panel is macOS-only".into(),
        ));
    }
    let status = Command::new("/usr/bin/open").arg(uri).status()?;
    if !status.success() {
        return Err(DesktopSystemError::Failed(
            "System Settings could not be opened".into(),
        ));
    }
    Ok(())
}

fn notification(success: bool) -> Result<(), DesktopSystemError> {
    let body = if success {
        "The native imaging operation completed. Review verification and hardware qualification."
    } else {
        "The imaging operation did not complete. Review the saved diagnostics; the target may be incomplete."
    };
    if cfg!(target_os = "macos") {
        let mut child = Command::new("/usr/bin/osascript")
            .args(["-", "WoR-Flasher", body])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()?;
        child.stdin.take().ok_or_else(||DesktopSystemError::Failed("Missing native notification input".into()))?
            .write_all(b"on run argv\n display notification (item 2 of argv) with title (item 1 of argv)\nend run\n")?;
        let output = child.wait_with_output()?;
        if !output.status.success() {
            return Err(DesktopSystemError::Failed(
                "macOS Notification Center request failed".into(),
            ));
        }
    } else {
        let status = Command::new("notify-send")
            .args(["WoR-Flasher", body])
            .status()?;
        if !status.success() {
            return Err(DesktopSystemError::Failed(
                "Desktop notification provider failed".into(),
            ));
        }
    }
    Ok(())
}

fn result_sound(macos: bool, success: bool, requested: &str) -> (&str, bool) {
    if !success {
        return (if macos { "Basso" } else { "dialog-error" }, false);
    }
    let (default, available): (&str, &[&str]) = if macos {
        (
            "Glass",
            &[
                "Basso",
                "Blow",
                "Bottle",
                "Frog",
                "Funk",
                "Glass",
                "Hero",
                "Morse",
                "Ping",
                "Pop",
                "Purr",
                "Sosumi",
                "Submarine",
                "Tink",
            ],
        )
    } else {
        (
            "complete",
            &[
                "complete",
                "bell",
                "message",
                "device-added",
                "dialog-information",
            ],
        )
    };
    if available.contains(&requested) {
        (requested, false)
    } else {
        (default, !requested.is_empty())
    }
}

fn sound(config: &NativeConfig, success: bool) -> Result<Option<String>, DesktopSystemError> {
    let macos = cfg!(target_os = "macos");
    let (event, fallback) = result_sound(macos, success, config.get("COMPLETION_SOUND"));
    if macos {
        let path = Path::new("/System/Library/Sounds").join(format!("{event}.aiff"));
        if !fs::metadata(&path).is_ok_and(|metadata| metadata.is_file()) {
            return Err(DesktopSystemError::Failed(
                "Selected system completion sound is unavailable".into(),
            ));
        }
        let status = Command::new("/usr/bin/afplay").arg(path).status()?;
        if !status.success() {
            return Err(DesktopSystemError::Failed(
                "System sound playback failed".into(),
            ));
        }
    } else {
        let playback = Command::new("canberra-gtk-play")
            .args(["--id", event])
            .status();
        let status = match playback {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let path =
                    Path::new("/usr/share/sounds/freedesktop/stereo").join(format!("{event}.oga"));
                if !fs::metadata(&path).is_ok_and(|metadata| metadata.is_file()) {
                    return Err(DesktopSystemError::Failed(
                        "No sound-theme player or freedesktop sound file is available".into(),
                    ));
                }
                Command::new("paplay").arg(path).status()?
            }
            result => result?,
        };
        if !status.success() {
            return Err(DesktopSystemError::Failed(
                "Sound-theme playback failed".into(),
            ));
        }
    }
    Ok(fallback
        .then(|| format!("Selected completion sound is unsupported on this host; used {event}.")))
}

pub fn completion_feedback(config: &NativeConfig, success: bool) -> Vec<String> {
    let mut warnings = Vec::new();
    match config.flag("PLAY_SOUND") {
        Ok(true) => match sound(config, success) {
            Ok(Some(warning)) => warnings.push(warning),
            Ok(None) => {}
            Err(error) => warnings.push(error.to_string()),
        },
        Ok(false) => {}
        Err(error) => warnings.push(error.to_string()),
    }
    match config.flag("SHOW_NOTIFICATION") {
        Ok(true) => {
            if let Err(error) = notification(success) {
                warnings.push(error.to_string());
            }
        }
        Ok(false) => {}
        Err(error) => warnings.push(error.to_string()),
    }
    warnings
}

fn version(value: &str) -> Option<(u64, u64, u64)> {
    let mut parts = value.strip_prefix('v').unwrap_or(value).split('.');
    let result = (
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    );
    parts.next().is_none().then_some(result)
}

pub fn release_notice(config: &NativeConfig) -> Result<Option<String>, String> {
    let slug = config.get("REPO_SLUG");
    let parts = slug.split('/').collect::<Vec<_>>();
    if parts.len() != 2
        || parts.iter().any(|part| {
            part.is_empty()
                || *part == "."
                || *part == ".."
                || !part
                    .bytes()
                    .all(|v| v.is_ascii_alphanumeric() || matches!(v, b'-' | b'_' | b'.'))
        })
    {
        return Err("Invalid release repository slug".into());
    }
    let current = version(config.version()).ok_or("Invalid current product version")?;
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent(format!("WoR-Flasher/{}", config.version()))
        .build()
        .map_err(|error| error.without_url().to_string())?;
    let response = client
        .get(format!(
            "https://api.github.com/repos/{slug}/releases/latest"
        ))
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|error| error.without_url().to_string())?;
    let data: serde_json::Value = response
        .json()
        .map_err(|error| error.without_url().to_string())?;
    if data.get("draft").and_then(serde_json::Value::as_bool) != Some(false)
        || data.get("prerelease").and_then(serde_json::Value::as_bool) != Some(false)
    {
        return Err("Latest stable release was not assessed".into());
    }
    let tag = data
        .get("tag_name")
        .and_then(serde_json::Value::as_str)
        .ok_or("Missing release tag")?;
    let latest = version(tag).ok_or("Invalid stable release tag")?;
    Ok((latest>current).then(||format!("A newer release {tag} is available at https://github.com/{slug}/releases/tag/{tag}. This check does not update the installation.")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_locale_preserves_platform_sources_and_nonempty_environment_precedence() {
        assert_eq!(
            locale_from_sources(true, Some("en_GB\n"), [Some("C"), None, Some("en_US")]),
            "en_GB"
        );
        assert_eq!(
            locale_from_sources(true, None, [Some("fr_FR"), None, None]),
            "en_US.UTF-8"
        );
        assert_eq!(
            locale_from_sources(
                false,
                Some("en_GB"),
                [Some("de_DE"), Some("fr_FR"), Some("es_ES")]
            ),
            "de_DE"
        );
        assert_eq!(
            locale_from_sources(false, None, [Some(""), Some("fr_FR"), Some("es_ES")]),
            "fr_FR"
        );
        assert_eq!(
            locale_from_sources(false, None, [Some(""), None, Some("es_ES")]),
            "es_ES"
        );
        assert_eq!(locale_from_sources(false, None, [None; 3]), "en_US.UTF-8");
    }

    #[test]
    fn sounds_preserve_platform_catalogues_fallbacks_and_fixed_failure_events() {
        assert_eq!(result_sound(true, true, "Ping"), ("Ping", false));
        assert_eq!(result_sound(false, true, "bell"), ("bell", false));
        assert_eq!(result_sound(true, true, ""), ("Glass", false));
        assert_eq!(result_sound(false, true, ""), ("complete", false));
        for unsupported in ["Glass", "unknown", "../../sound", "--help", "bad\nname"] {
            assert_eq!(result_sound(false, true, unsupported), ("complete", true));
        }
        assert_eq!(result_sound(true, true, "complete"), ("Glass", true));
        assert_eq!(result_sound(true, false, "../../sound"), ("Basso", false));
        assert_eq!(result_sound(false, false, "bell"), ("dialog-error", false));
    }

    #[test]
    fn stable_release_comparison_does_not_guess_invalid_or_prerelease_versions() {
        assert_eq!(version("v2.1.0"), Some((2, 1, 0)));
        assert_eq!(version("2.10.0"), Some((2, 10, 0)));
        for value in ["2.1", "2.1.0.1", "v2.1.0-rc1", "../tag"] {
            assert!(version(value).is_none());
        }
    }
}
