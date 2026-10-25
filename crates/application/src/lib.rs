use std::ffi::OsString;
use std::path::PathBuf;
use std::str::FromStr;
use thiserror::Error;
use wor_domain::progress::Progress;
pub mod wizard;

#[derive(Clone, PartialEq, Eq)]
pub struct Setting {
    name: String,
    value: String,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SettingError {
    #[error("Expected NAME=VALUE for --set")]
    MissingAssignment,
    #[error("Invalid environment variable name for --set")]
    InvalidName,
    #[error("An environment setting cannot contain NUL")]
    InvalidValue,
}

impl FromStr for Setting {
    type Err = SettingError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (name, value) = value
            .split_once('=')
            .ok_or(SettingError::MissingAssignment)?;
        let mut bytes = name.bytes();
        if !bytes
            .next()
            .is_some_and(|v| v.is_ascii_alphabetic() || v == b'_')
            || !bytes.all(|v| v.is_ascii_alphanumeric() || v == b'_')
        {
            return Err(SettingError::InvalidName);
        }
        if value.contains('\0') {
            return Err(SettingError::InvalidValue);
        }
        Ok(Self {
            name: name.to_owned(),
            value: value.to_owned(),
        })
    }
}

impl Setting {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn value(&self) -> &str {
        &self.value
    }

    pub fn assignment(&self) -> OsString {
        format!("{}={}", self.name, self.value).into()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Operation {
    ListDevices,
    DeviceInventory,
    DescribeDevice(PathBuf),
    Summary,
    Preview,
    Catalog(String),
    IotAccount(Vec<OsString>),
    Run(Vec<OsString>),
    ClassicWizard,
}

impl Operation {
    pub fn may_write(&self) -> bool {
        matches!(self, Self::Run(_) | Self::ClassicWizard)
    }
}

#[derive(Clone)]
pub struct Request {
    pub operation: Operation,
    pub config: Option<PathBuf>,
    pub expected_config_digest: Option<String>,
    pub settings: Vec<Setting>,
    pub progress_file: Option<PathBuf>,
    pub graphical: bool,
    pub sudo_password: Option<String>,
    pub private_input: Option<String>,
    pub allow_native_write: bool,
    pub target_fingerprint: Option<String>,
}

impl Request {
    pub fn new(operation: Operation) -> Self {
        Self {
            operation,
            config: None,
            expected_config_digest: None,
            settings: Vec::new(),
            progress_file: None,
            graphical: false,
            sudo_password: None,
            private_input: None,
            allow_native_write: false,
            target_fingerprint: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum EngineEvent {
    Progress(Progress),
    Diagnostic(String),
}

#[derive(Debug)]
pub struct Outcome {
    pub exit_code: i32,
    pub cancelled: bool,
    pub stdout: String,
    pub stderr: String,
}

impl Outcome {
    pub fn succeeded(&self) -> bool {
        self.exit_code == 0 && !self.cancelled
    }

    pub fn label(&self, dry_run: bool) -> String {
        if self.cancelled {
            "Canceled; any interrupted target must be flashed again.".into()
        } else if self.exit_code != 0 {
            format!(
                "Engine failed (exit {}). Review the diagnostics.",
                self.exit_code
            )
        } else if dry_run {
            "Dry run completed; no drive was written.".into()
        } else {
            "Engine completed. Review its verification and personalization results.".into()
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selection {
    pub config: PathBuf,
    pub device: PathBuf,
    pub dry_run: bool,
}

#[derive(Clone)]
pub struct Preview {
    pub selection: Selection,
    pub config_digest: String,
    pub summary: String,
    pub target_fingerprint: Option<String>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ConfirmationError {
    #[error("Choose a configuration file and target drive before previewing")]
    MissingSelection,
    #[error("The configuration or target changed. Preview the settings again")]
    ChangedSelection,
    #[error("Explicit erase confirmation is required")]
    EraseNotConfirmed,
}

impl Selection {
    pub fn validate(&self) -> Result<(), ConfirmationError> {
        if self.config.as_os_str().is_empty() || self.device.as_os_str().is_empty() {
            return Err(ConfirmationError::MissingSelection);
        }
        Ok(())
    }
}

impl Preview {
    pub fn authorize(
        &self,
        selection: &Selection,
        current_digest: &str,
        confirmed: bool,
    ) -> Result<(), ConfirmationError> {
        selection.validate()?;
        if &self.selection != selection || self.config_digest != current_digest {
            return Err(ConfirmationError::ChangedSelection);
        }
        if !selection.dry_run && !confirmed {
            return Err(ConfirmationError::EraseNotConfirmed);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_assignment_values_without_evaluating_shell_syntax() {
        for value in [
            "DEVICE=/dev/does-not-exist",
            "SOURCE_FILE=/tmp/image with spaces.iso",
            "CONFIG_TXT=x=y\nz=1",
            "NAME=literal; $(not-a-command)",
            "PASSWORD=",
        ] {
            let setting: Setting = value.parse().expect("valid assignment");
            assert_eq!(setting.assignment(), OsString::from(value));
        }
    }

    #[test]
    fn rejects_bad_assignments_without_echoing_potential_secrets() {
        for value in [
            "PASSWORD-secret",
            "BAD-NAME=secret",
            "1NAME=secret",
            "=secret",
            "X=secret\0",
        ] {
            let error = value.parse::<Setting>().err().expect("invalid setting");
            assert!(!error.to_string().contains("secret"));
        }
    }

    fn preview() -> Preview {
        Preview {
            selection: Selection {
                config: "/tmp/config with spaces.json".into(),
                device: "/dev/does-not-exist".into(),
                dry_run: false,
            },
            config_digest: "fixture-digest".into(),
            summary: "fixture".into(),
            target_fingerprint: None,
        }
    }

    #[test]
    fn binds_erase_consent_to_the_previewed_configuration_and_target() {
        let preview = preview();
        assert!(
            preview
                .authorize(&preview.selection, "fixture-digest", true)
                .is_ok()
        );
        assert_eq!(
            preview.authorize(&preview.selection, "fixture-digest", false),
            Err(ConfirmationError::EraseNotConfirmed)
        );
        assert_eq!(
            preview.authorize(&preview.selection, "changed", true),
            Err(ConfirmationError::ChangedSelection)
        );
        let mut changed = preview.selection.clone();
        changed.device = "/dev/another-missing-device".into();
        assert_eq!(
            preview.authorize(&changed, "fixture-digest", true),
            Err(ConfirmationError::ChangedSelection)
        );
        changed = preview.selection.clone();
        changed.dry_run = true;
        assert_eq!(
            preview.authorize(&changed, "fixture-digest", true),
            Err(ConfirmationError::ChangedSelection)
        );
    }

    #[test]
    fn does_not_present_a_failed_or_canceled_run_as_verified() {
        for (code, cancelled) in [(1, false), (0, true), (130, true)] {
            let outcome = Outcome {
                exit_code: code,
                cancelled,
                stdout: String::new(),
                stderr: String::new(),
            };
            assert!(!outcome.succeeded());
            assert!(!outcome.label(false).contains("verified"));
        }
        let outcome = Outcome {
            exit_code: 0,
            cancelled: false,
            stdout: String::new(),
            stderr: String::new(),
        };
        assert!(outcome.label(true).contains("no drive was written"));
        assert!(!outcome.label(false).contains("verified"));
    }
}
