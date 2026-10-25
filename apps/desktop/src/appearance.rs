use anyhow::{Context, Result, bail};
use serde_json::json;
use std::fmt;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

impl Appearance {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }
}

impl fmt::Display for Appearance {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for Appearance {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "system" => Ok(Self::System),
            "light" => Ok(Self::Light),
            "dark" => Ok(Self::Dark),
            _ => bail!("Unsupported appearance; choose System, Light or Dark."),
        }
    }
}

pub fn preference_path(home: &Path, macos: bool) -> PathBuf {
    if macos {
        home.join("Library/Application Support/WoR-Flasher/appearance.json")
    } else {
        home.join(".config/wor-flasher/appearance.json")
    }
}

pub fn load(path: &Path) -> Result<Appearance> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Appearance::System);
        }
        Err(error) => return Err(error).context("Cannot inspect saved appearance."),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 1024 {
        bail!("Saved appearance must be a bounded regular non-symlink file.");
    }
    let mut bytes = Vec::new();
    File::open(path)?.take(1025).read_to_end(&mut bytes)?;
    if bytes.len() > 1024 {
        bail!("Saved appearance exceeded its size bound.");
    }
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).context("Saved appearance is not valid JSON.")?;
    if value.get("schemaVersion") != Some(&json!(1))
        || value.as_object().is_none_or(|object| object.len() != 2)
    {
        bail!("Saved appearance has an unsupported schema.");
    }
    value
        .get("appearance")
        .and_then(serde_json::Value::as_str)
        .context("Saved appearance is missing its mode.")?
        .parse()
}

pub fn save(path: &Path, appearance: Appearance) -> Result<()> {
    let parent = path.parent().context("Appearance path has no parent.")?;
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(parent)?;
    let metadata = fs::symlink_metadata(parent)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        bail!("Appearance directory must not be a symlink.");
    }
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.is_file() || metadata.file_type().is_symlink() => {
            bail!("Refusing to replace a non-regular appearance file.");
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("Cannot inspect the existing appearance file."),
    }
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer(
        &mut temporary,
        &json!({"schemaVersion":1,"appearance":appearance.as_str()}),
    )?;
    temporary.write_all(b"\n")?;
    temporary.flush()?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(path)
        .map_err(|error| error.error)
        .context("Cannot atomically save appearance.")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_is_default_and_only_known_modes_are_accepted() {
        assert_eq!(Appearance::default(), Appearance::System);
        for mode in [Appearance::System, Appearance::Light, Appearance::Dark] {
            assert_eq!(mode.as_str().parse::<Appearance>().unwrap(), mode);
        }
        for value in ["", "automatic", "DARK", "../dark"] {
            assert!(value.parse::<Appearance>().is_err());
        }
    }

    #[test]
    fn selection_survives_reload_without_touching_run_configuration() {
        let root = tempfile::tempdir().unwrap();
        let path = preference_path(root.path(), false);
        assert_eq!(load(&path).unwrap(), Appearance::System);
        for mode in [Appearance::Dark, Appearance::Light, Appearance::System] {
            save(&path, mode).unwrap();
            assert_eq!(load(&path).unwrap(), mode);
        }
        assert!(!root.path().join("config.json").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn corrupt_unknown_and_oversized_preferences_are_errors_not_silent_defaults() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("appearance.json");
        for text in [
            "{",
            r#"{"schemaVersion":2,"appearance":"dark"}"#,
            r#"{"schemaVersion":1,"appearance":"unknown"}"#,
            r#"{"schemaVersion":1,"appearance":"dark","extra":true}"#,
        ] {
            fs::write(&path, text).unwrap();
            assert!(load(&path).is_err());
        }
        fs::write(&path, vec![b' '; 1025]).unwrap();
        assert!(load(&path).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn preference_links_are_never_followed_or_overwritten() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("unrelated");
        fs::write(&target, "unchanged").unwrap();
        let path = root.path().join("appearance.json");
        symlink(&target, &path).unwrap();
        assert!(load(&path).is_err());
        assert!(save(&path, Appearance::Dark).is_err());
        assert_eq!(fs::read_to_string(target).unwrap(), "unchanged");
    }
}
