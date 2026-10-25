use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            assert!(entry.file_type().unwrap().is_file());
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn cargo(root: &Path, target: &Path, test: Option<&str>) -> Output {
    let mut command = Command::new(env!("CARGO"));
    command.args([
        "test",
        "--offline",
        "-p",
        "wor-desktop-media",
        "--test",
        "preparation",
    ]);
    if let Some(test) = test {
        command.args([test, "--", "--exact"]);
    }
    command
        .env("CARGO_TARGET_DIR", target)
        .env("TMPDIR", root)
        .current_dir(root)
        .output()
        .unwrap()
}

#[test]
#[ignore = "explicit offline isolated-copy mutation check; see crate README"]
fn isolated_desktop_guard_mutations_are_killed() {
    let owned = Path::new(env!("CARGO_MANIFEST_DIR"));
    let repository = owned.join("../..").canonicalize().unwrap();
    let work = owned.join("target/test-work");
    fs::create_dir_all(&work).unwrap();
    let fixture = tempfile::Builder::new()
        .prefix("guard-mutations-")
        .tempdir_in(&work)
        .unwrap();
    let root = fixture.path();
    let desktop = root.join("crates/desktop-media");
    copy_tree(&owned.join("src"), &desktop.join("src"));
    fs::create_dir_all(desktop.join("tests")).unwrap();
    fs::copy(owned.join("Cargo.toml"), desktop.join("Cargo.toml")).unwrap();
    fs::copy(
        owned.join("tests/preparation.rs"),
        desktop.join("tests/preparation.rs"),
    )
    .unwrap();
    fs::copy(repository.join("Cargo.lock"), root.join("Cargo.lock")).unwrap();
    copy_tree(&repository.join("src/config"), &root.join("src/config"));
    copy_tree(
        &repository.join("config-templates"),
        &root.join("config-templates"),
    );
    copy_tree(
        &repository.join("src/lib/pi3-boot-refresh"),
        &root.join("src/lib/pi3-boot-refresh"),
    );
    let domain = serde_json::to_string(repository.join("crates/domain").to_str().unwrap()).unwrap();
    fs::write(root.join("Cargo.toml"), format!(r#"
[workspace]
members = ["crates/desktop-media"]
resolver = "2"
[workspace.package]
version = "0.1.0"
edition = "2024"
rust-version = "1.99"
license = "GPL-3.0-or-later"
publish = false
[workspace.dependencies]
fs2 = "0.4"
serde = {{ version = "1", features = ["derive"] }}
serde_json = "1"
sha2 = "0.10"
reqwest = {{ version = "0.12", default-features = false, features = ["blocking", "json", "rustls-tls"] }}
tempfile = "3"
thiserror = "2"
wor-domain = {{ path = {domain} }}
[workspace.lints.rust]
unsafe_op_in_unsafe_fn = "deny"
"#)).unwrap();
    let target = owned.join("target/mutation-build");
    let baseline = cargo(root, &target, None);
    assert!(
        baseline.status.success(),
        "isolated baseline failed:\n{}\n{}",
        String::from_utf8_lossy(&baseline.stdout),
        String::from_utf8_lossy(&baseline.stderr)
    );
    let mutations = [
        (
            "src/options.rs",
            "self == Self::Pi5 || build.major() <= armv80_ceiling",
            "{ let _ = (self, build, armv80_ceiling); true }",
            "safe_build_and_model_guards_run_before_downloads_or_native_tools",
        ),
        (
            "src/wim.rs",
            "if info.compression != \"LZX\" || info.chunk_size != 32768 || blobs.solid {",
            "if false {",
            "every_conversion_failure_preserves_base_and_cleans_private_staging",
        ),
        (
            "src/setup.rs",
            "if !sha256_file(&path, cancelled)?.eq_ignore_ascii_case(digest) {",
            "if false {",
            "pi3_qualified_go_helper_digest_is_required_even_when_trusting_cache",
        ),
        (
            "src/wim.rs",
            "image.architecture != 12 || image.build != Some(major)",
            "image.build != Some(major)",
            "actual_image_architecture_and_build_cannot_bypass_selected_model_guards",
        ),
    ];
    for (file, needle, replacement, test) in mutations {
        let path = desktop.join(file);
        let original = fs::read_to_string(&path).unwrap();
        assert_eq!(
            original.matches(needle).count(),
            1,
            "mutation anchor changed: {file}"
        );
        fs::write(&path, original.replacen(needle, replacement, 1)).unwrap();
        let result = cargo(root, &target, Some(test));
        fs::write(&path, original).unwrap();
        let stdout = String::from_utf8_lossy(&result.stdout);
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(
            !result.status.success()
                && stdout.contains("1 failed")
                && stdout.contains(test)
                && !stderr.contains("error[E"),
            "guard mutation survived or failed to compile: {test}\n{stdout}\n{stderr}"
        );
        eprintln!("Killed isolated guard mutation: {test}");
    }
    fixture.close().unwrap();
}
