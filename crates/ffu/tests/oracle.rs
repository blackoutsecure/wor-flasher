//! Python is used only here as the frozen, offline test oracle, never by the library.
use std::{
    ffi::OsString,
    fs,
    io::Cursor,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::AtomicBool,
};

use serde_json::Value;
use sha2::{Digest, Sha256};
use wor_ffu::{
    ApplyOptions, apply_to_file_fixture, hdmi_options, image_profile, parse_ffu, resolve_plan,
};

struct Corpus(PathBuf);
impl Corpus {
    fn new() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join(".test-state");
        fs::create_dir_all(&root).unwrap();
        let path = root.join(format!("oracle-{}", std::process::id()));
        fs::create_dir(&path).unwrap();
        Self(path.canonicalize().unwrap())
    }
}
impl Drop for Corpus {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
        let _ = fs::remove_dir(self.0.parent().unwrap());
    }
}

fn path(case: &Value, key: &str) -> PathBuf {
    PathBuf::from(case[key].as_str().unwrap())
}

fn normalized_info(info: &wor_ffu::ParsedFfu) -> Value {
    let mut value = serde_json::to_value(info).unwrap();
    let hex: String = info
        .chunk_hashes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    value["chunk_hashes"] = Value::String(hex);
    value
}

#[test]
fn complete_python_source_oracle_corpus() {
    let corpus = Corpus::new();
    let oracle = Command::new("python3")
        .arg("-B")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/python_oracle.py"))
        .arg(&corpus.0)
        .env("WOR_TEST_NATIVE_LOOP", "0")
        .output()
        .expect("python3 is required to replay the unchanged source oracle");
    assert!(
        oracle.status.success(),
        "oracle failed: {}",
        String::from_utf8_lossy(&oracle.stderr)
    );
    let data: Value = serde_json::from_slice(&oracle.stdout).unwrap();
    let parser = data["parser"].as_array().unwrap();
    assert!(parser.len() >= 200);
    for case in parser {
        let blob = fs::read(path(case, "path")).unwrap();
        let result = parse_ffu(&mut Cursor::new(blob)).and_then(|info| {
            let plan = resolve_plan(&info, case["target_size"].as_u64().unwrap())?;
            Ok((info, plan))
        });
        match result {
            Ok((info, plan)) => {
                assert_eq!(
                    case["status"], 0,
                    "unexpected native accept: {}",
                    case["label"]
                );
                assert_eq!(
                    normalized_info(&info),
                    case["info"],
                    "info: {}",
                    case["label"]
                );
                assert_eq!(
                    serde_json::to_value(plan).unwrap(),
                    case["plan"],
                    "plan: {}",
                    case["label"]
                );
            }
            Err(error) => {
                assert_eq!(
                    error.exit_status(),
                    case["status"].as_i64().unwrap() as i32,
                    "native/oracle divergence {}: {error}; oracle {}",
                    case["label"],
                    case["error"]
                );
            }
        }
    }
    let writers = data["writer"].as_array().unwrap();
    assert!(writers.len() >= 30);
    for (index, case) in writers.iter().enumerate() {
        let target = corpus.0.join(format!("rust-target-{index:03}.raw"));
        fs::copy(path(case, "initial"), &target).unwrap();
        let options = &case["options"];
        let text = |name: &str| options[name].as_str().map(str::to_owned);
        let options = ApplyOptions {
            target_size: case["target_size"].as_u64().unwrap(),
            expected_sha256: text("expected_sha256"),
            expected_target_id: text("expected_target_id"),
            allow_gpt_cleanup: options["allow_gpt_cleanup"].as_bool().unwrap_or(false),
            wipe_entire_drive: options["wipe_entire_drive"].as_bool().unwrap_or(false),
            hdmi_mode: text("hdmi_mode").unwrap_or_else(|| "official".into()),
            hdmi_config: text("hdmi_config").unwrap_or_default(),
        };
        let mut progress = Vec::new();
        let source = path(case, "source");
        let result = apply_to_file_fixture(
            &source,
            &target,
            &options,
            Some(&mut |phase, percent| progress.push((phase.to_string(), percent))),
            &AtomicBool::new(false),
        );
        let status = result.map_or_else(|error| error.exit_status(), |_| 0);
        assert_eq!(
            status as i64,
            case["status"].as_i64().unwrap(),
            "writer: {}",
            case["label"]
        );
        assert_eq!(
            fs::read(&target).unwrap(),
            fs::read(path(case, "expected")).unwrap(),
            "bytes: {}",
            case["label"]
        );
        assert_eq!(
            serde_json::to_value(progress).unwrap(),
            case["progress"],
            "progress: {}",
            case["label"]
        );
        assert_eq!(
            format!("{:x}", Sha256::digest(fs::read(&source).unwrap())),
            case["source_sha256_after"].as_str().unwrap()
        );
    }
    for case in data["hdmi"].as_array().unwrap() {
        let mode = case["mode"].as_str().unwrap();
        let custom = case["custom"].as_str().unwrap();
        assert_eq!(
            serde_json::to_value(hdmi_options(mode, custom).unwrap()).unwrap(),
            case["expected"]
        );
        let args = [
            "hdmi-options",
            "--hdmi-mode",
            mode,
            "--hdmi-config",
            custom,
            "--json",
        ]
        .map(OsString::from);
        let (mut out, mut err) = (Vec::new(), Vec::new());
        assert_eq!(wor_ffu::run_with_io(&args, &mut out, &mut err).unwrap(), 0);
        assert!(err.is_empty());
        assert_eq!(
            serde_json::from_slice::<Value>(&out).unwrap(),
            case["expected"]
        );
    }
    for (board, expected) in data["profiles"].as_object().unwrap() {
        assert_eq!(
            serde_json::to_value(image_profile(board).unwrap()).unwrap(),
            *expected
        );
    }
    let identity = &data["native_identity"];
    let options = ApplyOptions {
        target_size: 32768,
        expected_target_id: Some(identity["target_id"].as_str().unwrap().into()),
        ..Default::default()
    };
    apply_to_file_fixture(
        &path(identity, "source"),
        &path(identity, "path"),
        &options,
        None,
        &AtomicBool::new(false),
    )
    .expect("native fstat device/inode/raw-device rendering must match Python exactly");
    eprintln!(
        "Python parity: {} parser/geometry cases, {} real-writer byte/progress cases, {} HDMI + {} profile cases",
        parser.len(),
        writers.len(),
        data["hdmi"].as_array().unwrap().len(),
        data["profiles"].as_object().unwrap().len()
    );
}
