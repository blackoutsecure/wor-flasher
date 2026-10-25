use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;

fn collect(root: &Path, path: &Path, files: &mut Vec<serde_json::Value>) {
    let metadata = fs::symlink_metadata(path).expect("Reading canonical runtime metadata");
    assert!(
        !metadata.file_type().is_symlink(),
        "Runtime symlinks are not supported"
    );
    if metadata.is_dir() {
        for entry in fs::read_dir(path).expect("Reading canonical runtime directory") {
            collect(root, &entry.expect("Reading runtime entry").path(), files);
        }
    } else {
        assert!(metadata.is_file(), "Runtime entries must be regular files");
        #[cfg(unix)]
        let mode = {
            use std::os::unix::fs::PermissionsExt;
            format!("{:o}", metadata.permissions().mode() & 0o777)
        };
        #[cfg(not(unix))]
        let mode = "644".to_owned();
        files.push(serde_json::json!({
            "path": path.strip_prefix(root).expect("Runtime path").to_string_lossy().replace('\\', "/"),
            "sha256": format!("{:x}", Sha256::digest(fs::read(path).expect("Reading canonical runtime file"))),
            "mode": mode,
        }));
    }
}

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let metadata_path = root.join("src/config/metadata.json");
    println!("cargo:rerun-if-changed={}", metadata_path.display());
    let metadata: serde_json::Value =
        serde_json::from_slice(&fs::read(&metadata_path).expect("Reading product metadata"))
            .expect("Parsing product metadata");
    let mut files = Vec::new();
    for path in metadata["nativeRuntimePaths"]
        .as_array()
        .expect("Native runtime inventory")
    {
        let path = root.join(path.as_str().expect("Runtime path must be a string"));
        println!("cargo:rerun-if-changed={}", path.display());
        collect(&root, &path, &mut files);
    }
    files.sort_by(|left, right| left["path"].as_str().cmp(&right["path"].as_str()));
    let manifest = serde_json::json!({
        "schemaVersion": 1,
        "version": metadata["product"]["version"],
        "files": files,
    });
    fs::write(
        Path::new(&std::env::var_os("OUT_DIR").expect("Cargo OUT_DIR"))
            .join("runtime-manifest.json"),
        serde_json::to_vec(&manifest).expect("Serializing runtime inventory"),
    )
    .expect("Writing compiled runtime inventory");
}
