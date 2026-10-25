use std::ffi::OsString;
use std::fs;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use wor_desktop_media::*;

fn workspace() -> tempfile::TempDir {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/test-work");
    fs::create_dir_all(&root).unwrap();
    tempfile::Builder::new()
        .prefix("native-provider-")
        .tempdir_in(root)
        .unwrap()
}

fn fixture_tool(root: &Path) -> PathBuf {
    let executable = root.join("native-tool with ' spaces");
    let result = Command::new("cc")
        .args(["-std=c11", "-Wall", "-Werror", "-o"])
        .arg(&executable)
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/fake-tool.c"))
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    executable
}

fn provider(executable: &Path) -> NativeProvider {
    NativeProvider::with_tools(NativeTools {
        wimlib: executable.into(),
        archive: executable.into(),
        mount: executable.into(),
    })
    .unwrap()
}

fn iso_header(path: &Path) {
    let mut file = fs::File::create(path).unwrap();
    file.set_len(40 * 2048).unwrap();
    file.seek(SeekFrom::Start(16 * 2048)).unwrap();
    file.write_all(b"\x01CD001\x01").unwrap();
}

#[test]
fn native_provider_passes_distinct_argv_and_inherits_worker_group() {
    let root = workspace();
    let executable = fixture_tool(root.path());
    let mut provider = provider(&executable);
    let args: Vec<OsString> = vec![
        root.path().join("source image;not a shell.wim").into(),
        "2".into(),
        "--command=add \"file with spaces\" \"/drivers\"".into(),
    ];
    provider
        .wim(
            WimOperation::Update,
            &args,
            root.path(),
            &AtomicBool::new(false),
        )
        .unwrap();
    let actual: Vec<_> = fs::read(root.path().join("native-argv"))
        .unwrap()
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8(s.into()).unwrap())
        .collect();
    assert_eq!(actual[0], "update");
    assert_eq!(actual[1], args[0].to_string_lossy());
    assert_eq!(actual[3], args[2].to_string_lossy());
    let parent_group = Command::new("ps")
        .args(["-o", "pgid=", "-p", &std::process::id().to_string()])
        .output()
        .unwrap();
    assert_eq!(
        fs::read_to_string(root.path().join("native-pgid"))
            .unwrap()
            .trim(),
        String::from_utf8(parent_group.stdout).unwrap().trim()
    );
}

#[test]
fn native_tool_diagnostics_and_secret_urls_never_escape_the_error_boundary() {
    let root = workspace();
    let executable = fixture_tool(root.path());
    fs::write(root.path().join("fail"), b"fail").unwrap();
    let error = provider(&executable)
        .wim(
            WimOperation::Verify,
            &[],
            root.path(),
            &AtomicBool::new(false),
        )
        .unwrap_err();
    assert!(matches!(
        error,
        MediaError::Tool {
            status: Some(34),
            ..
        }
    ));
    for text in [error.to_string(), format!("{error:?}")] {
        assert!(!text.contains("secret-never-log"));
        assert!(!text.contains("https://"));
    }
}

#[test]
fn cancellation_kills_and_reaps_the_owned_native_child_without_detaching_it() {
    let root = workspace();
    let executable = fixture_tool(root.path());
    fs::write(root.path().join("delay"), b"delay").unwrap();
    let cancelled = Arc::new(AtomicBool::new(false));
    let flag = cancelled.clone();
    let thread = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(150));
        flag.store(true, Ordering::Release);
    });
    let start = Instant::now();
    let result = provider(&executable).wim(WimOperation::Verify, &[], root.path(), &cancelled);
    thread.join().unwrap();
    assert!(matches!(result, Err(MediaError::Cancelled)), "{result:?}");
    assert!(start.elapsed() < Duration::from_secs(5));
}

#[test]
fn private_readonly_iso_mount_is_cleaned_on_success() {
    let root = workspace();
    let executable = fixture_tool(root.path());
    let image = root.path().join("source.iso");
    iso_header(&image);
    let output = root.path().join("iso-files");
    provider(&executable)
        .extract_iso(&image, &output, &AtomicBool::new(false))
        .unwrap();
    assert_eq!(
        fs::read(output.join("sources/install.esd")).unwrap(),
        b"fixture install"
    );
    assert!(!fs::read_dir(root.path()).unwrap().any(|e| {
        e.unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".iso-mount-")
    }));
    let argv = fs::read(root.path().join("native-argv")).unwrap();
    assert!(
        String::from_utf8_lossy(&argv).contains(if cfg!(target_os = "macos") {
            "-readonly"
        } else {
            "loop,ro,nosuid,nodev,noexec"
        })
    );
    assert!(
        String::from_utf8_lossy(&argv).contains(if cfg!(target_os = "macos") {
            "detach"
        } else {
            "umount"
        })
    );
}

#[test]
fn failed_iso_unmount_is_an_error_and_retains_only_private_mount_state() {
    let root = workspace();
    let executable = fixture_tool(root.path());
    let image = root.path().join("source.iso");
    iso_header(&image);
    fs::write(root.path().join("fail-detach"), "fixture").unwrap();
    let result = provider(&executable).extract_iso(
        &image,
        &root.path().join("iso-files"),
        &AtomicBool::new(false),
    );
    assert!(
        matches!(result, Err(MediaError::Cleanup { .. })),
        "{result:?}"
    );
    let mounts: Vec<_> = fs::read_dir(root.path())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(".iso-mount-")
        })
        .collect();
    assert_eq!(mounts.len(), 1);
    assert!(mounts[0].path().join("sources/install.esd").is_file());
    // This fixture never mounts anything; cleanup is safe only for this fake.
    fs::remove_dir_all(mounts[0].path()).unwrap();
}

#[test]
fn files_disguised_as_iso_do_not_reach_archive_or_mount_tools() {
    let root = workspace();
    let executable = fixture_tool(root.path());
    let image = root.path().join("not-an-iso.iso");
    fs::write(&image, b"not an image").unwrap();
    assert!(
        provider(&executable)
            .extract_iso(&image, &root.path().join("output"), &AtomicBool::new(false))
            .is_err()
    );
    assert!(!root.path().join("native-argv").exists());
}

#[test]
fn linux_readonly_mount_uses_fixed_argv_and_cleans_both_success_and_retry_failure() {
    for fail in [false, true] {
        let root = workspace();
        let executable = fixture_tool(root.path());
        let image = root.path().join("linux-source.iso");
        iso_header(&image);
        if fail {
            fs::write(root.path().join("fail-attach"), "fixture").unwrap();
        }
        let mut native = provider(&executable).with_mount_style(MountStyle::Linux);
        let result = native.extract_iso(
            &image,
            &root.path().join("iso-files"),
            &AtomicBool::new(false),
        );
        assert_eq!(result.is_err(), fail, "{result:?}");
        let args = fs::read(root.path().join("native-argv")).unwrap();
        let args = String::from_utf8_lossy(&args);
        assert!(args.contains("-n\0mount\0-o\0loop,ro,nosuid,nodev,noexec\0--\0"));
        assert!(args.contains("-n\0umount\0--\0"));
        if fail {
            assert!(args.contains("-n\0modprobe\0udf\0"));
        }
        assert!(!fs::read_dir(root.path()).unwrap().any(|e| {
            e.unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".iso-mount-")
        }));
    }
}
