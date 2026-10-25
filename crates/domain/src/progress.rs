use thiserror::Error;

#[derive(Clone, Debug, PartialEq)]
pub enum Progress {
    Status(String),
    Step {
        current: u32,
        total: u32,
        message: String,
    },
    Substep(f64),
    Task {
        percent: f64,
        label: String,
    },
    DiskWrite {
        active: bool,
        device: String,
    },
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProgressError {
    #[error("Unknown engine progress record")]
    UnknownRecord,
    #[error("Malformed engine progress record")]
    MalformedRecord,
    #[error("Engine progress percentage is outside 0..=100")]
    InvalidPercentage,
    #[error("Engine step must have a positive total and current <= total")]
    InvalidStep,
}

fn percent(value: &str) -> Result<f64, ProgressError> {
    let number: f64 = value
        .parse()
        .map_err(|_| ProgressError::InvalidPercentage)?;
    if !number.is_finite() || !(0.0..=100.0).contains(&number) {
        return Err(ProgressError::InvalidPercentage);
    }
    Ok(number)
}

impl Progress {
    pub fn parse(line: &str) -> Result<Self, ProgressError> {
        let line = line.trim_end_matches(['\r', '\n']);
        let (kind, value) = line
            .split_once('\t')
            .ok_or(ProgressError::MalformedRecord)?;
        match kind {
            "DISK_WRITE" => {
                let (active, device) = value
                    .split_once('\t')
                    .ok_or(ProgressError::MalformedRecord)?;
                if !matches!(active, "0" | "1") || !device.starts_with("/dev/") {
                    return Err(ProgressError::MalformedRecord);
                }
                Ok(Self::DiskWrite {
                    active: active == "1",
                    device: device.into(),
                })
            }
            "STATUS" => Ok(Self::Status(value.to_owned())),
            "SUBSTEP" => Ok(Self::Substep(percent(value)?)),
            "TASK" => {
                let (number, label) = value
                    .split_once('\t')
                    .ok_or(ProgressError::MalformedRecord)?;
                Ok(Self::Task {
                    percent: percent(number)?,
                    label: label.to_owned(),
                })
            }
            "STEP" => {
                let mut values = value.splitn(3, '\t');
                let current = values
                    .next()
                    .and_then(|v| v.parse::<u32>().ok())
                    .ok_or(ProgressError::MalformedRecord)?;
                let total = values
                    .next()
                    .and_then(|v| v.parse::<u32>().ok())
                    .ok_or(ProgressError::MalformedRecord)?;
                let message = values
                    .next()
                    .ok_or(ProgressError::MalformedRecord)?
                    .to_owned();
                if total == 0 || current > total {
                    return Err(ProgressError::InvalidStep);
                }
                Ok(Self::Step {
                    current,
                    total,
                    message,
                })
            }
            _ => Err(ProgressError::UnknownRecord),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_existing_tab_protocol_without_parsing_terminal_output() {
        assert_eq!(
            Progress::parse("STATUS\tPreparing Windows\timage\r\n"),
            Ok(Progress::Status("Preparing Windows\timage".into()))
        );
        assert_eq!(
            Progress::parse("STEP\t2\t6\tVerify image"),
            Ok(Progress::Step {
                current: 2,
                total: 6,
                message: "Verify image".into()
            })
        );
        assert_eq!(
            Progress::parse("TASK\t84\tinstall.wim"),
            Ok(Progress::Task {
                percent: 84.0,
                label: "install.wim".into()
            })
        );
        assert!(Progress::parse("TASK\t0\t").is_ok());
        assert_eq!(
            Progress::parse("DISK_WRITE\t1\t/dev/fixture"),
            Ok(Progress::DiskWrite {
                active: true,
                device: "/dev/fixture".into()
            })
        );
        assert_eq!(Progress::parse("SUBSTEP\t0.5"), Ok(Progress::Substep(0.5)));
    }

    #[test]
    fn rejects_malformed_or_non_finite_progress_explicitly() {
        for line in [
            "",
            "STATUS",
            "STEP\t2\t1\tx",
            "STEP\t0\t0\tx",
            "STEP\t1\t2",
            "SUBSTEP\tNaN",
            "SUBSTEP\tinf",
            "SUBSTEP\t-1",
            "SUBSTEP\t101",
            "TASK\t50",
            "UNKNOWN\tx",
        ] {
            assert!(Progress::parse(line).is_err(), "{line}");
        }
        for boundary in ["0", "100"] {
            assert!(Progress::parse(&format!("SUBSTEP\t{boundary}")).is_ok());
        }
    }
}
