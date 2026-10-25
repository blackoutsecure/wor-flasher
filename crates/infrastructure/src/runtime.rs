use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use thiserror::Error;

const CANONICAL_MANIFEST: &str = include_str!(concat!(env!("OUT_DIR"), "/runtime-manifest.json"));
const PRODUCT_METADATA: &str = include_str!("../../../src/config/metadata.json");

#[derive(Clone, Debug, Deserialize)]
pub struct Product {
    pub name: String,
    pub version: String,
}

#[derive(Deserialize)]
struct Metadata {
    product: Product,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ManifestFile {
    path: String,
    sha256: String,
    mode: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Manifest {
    schema_version: u32,
    version: String,
    files: Vec<ManifestFile>,
}

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("Cannot read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("Invalid runtime metadata: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Incomplete or unsafe runtime path: {0}")]
    UnsafePath(PathBuf),
    #[error(
        "Runtime manifest does not match this executable; rebuild or reinstall the complete release"
    )]
    ManifestMismatch,
    #[error("Runtime content or permissions changed: {0}")]
    ContentMismatch(PathBuf),
    #[error("No packaged runtime found; use --runtime-dir with a trusted complete source checkout")]
    MissingRuntime,
    #[error("Configuration must be a JSON object no larger than 1 MiB")]
    InvalidConfig,
}

fn io(path: &Path, source: std::io::Error) -> RuntimeError {
    RuntimeError::Io {
        path: path.to_owned(),
        source,
    }
}

pub fn bundled_product() -> Result<Product, RuntimeError> {
    Ok(serde_json::from_str::<Metadata>(PRODUCT_METADATA)?.product)
}

fn compiled_manifest() -> Result<Manifest, RuntimeError> {
    Ok(serde_json::from_str(CANONICAL_MANIFEST)?)
}

fn safe_relative(path: &str) -> bool {
    !path.is_empty()
        && !path.contains('\\')
        && Path::new(path)
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
}

fn regular_path(root: &Path, relative: &str) -> Result<PathBuf, RuntimeError> {
    if !safe_relative(relative) {
        return Err(RuntimeError::UnsafePath(relative.into()));
    }
    let mut path = root.to_owned();
    for component in Path::new(relative).components() {
        path.push(component);
        let metadata = fs::symlink_metadata(&path).map_err(|e| io(&path, e))?;
        if metadata.file_type().is_symlink() {
            return Err(RuntimeError::UnsafePath(path));
        }
    }
    if !path.is_file() {
        return Err(RuntimeError::UnsafePath(path));
    }
    Ok(path)
}

fn enumerate(
    root: &Path,
    directory: &Path,
    files: &mut BTreeSet<String>,
) -> Result<(), RuntimeError> {
    for entry in fs::read_dir(directory).map_err(|e| io(directory, e))? {
        let path = entry.map_err(|e| io(directory, e))?.path();
        let metadata = fs::symlink_metadata(&path).map_err(|e| io(&path, e))?;
        if metadata.file_type().is_symlink() {
            return Err(RuntimeError::UnsafePath(path));
        }
        if metadata.is_dir() {
            enumerate(root, &path, files)?;
        } else if metadata.is_file() {
            files.insert(
                path.strip_prefix(root)
                    .map_err(|_| RuntimeError::UnsafePath(path.clone()))?
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        } else {
            return Err(RuntimeError::UnsafePath(path));
        }
    }
    Ok(())
}

#[derive(Clone)]
pub struct Runtime {
    root: PathBuf,
    manifest: Option<PathBuf>,
}

impl Runtime {
    pub fn resolve(explicit: Option<&Path>, executable: &Path) -> Result<Self, RuntimeError> {
        if let Some(root) = explicit {
            return Self::source(root);
        }
        let binaries = executable.parent().ok_or(RuntimeError::MissingRuntime)?;
        let parent = binaries.parent().ok_or(RuntimeError::MissingRuntime)?;
        let package = if binaries.file_name().is_some_and(|v| v == "MacOS") {
            parent.join("Resources")
        } else {
            parent.to_owned()
        };
        let manifest = package.join("runtime-manifest.json");
        let root = package.join("runtime");
        if !root.is_dir() || !manifest.is_file() {
            return Err(RuntimeError::MissingRuntime);
        }
        Self::verified(&root, &manifest)
    }

    pub fn source(root: &Path) -> Result<Self, RuntimeError> {
        let metadata = fs::symlink_metadata(root).map_err(|e| io(root, e))?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(RuntimeError::UnsafePath(root.to_owned()));
        }
        let runtime = Self {
            root: root.canonicalize().map_err(|e| io(root, e))?,
            manifest: None,
        };
        runtime.validate()?;
        Ok(runtime)
    }

    pub fn verified(root: &Path, manifest: &Path) -> Result<Self, RuntimeError> {
        let runtime = Self {
            root: root.to_owned(),
            manifest: Some(manifest.to_owned()),
        };
        runtime.validate()?;
        Ok(runtime)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn manifest_path(&self) -> Option<&Path> {
        self.manifest.as_deref()
    }

    pub fn validate(&self) -> Result<(), RuntimeError> {
        let root_metadata = fs::symlink_metadata(&self.root).map_err(|e| io(&self.root, e))?;
        if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
            return Err(RuntimeError::UnsafePath(self.root.clone()));
        }
        let expected = compiled_manifest()?;
        let expected_paths: BTreeSet<_> = expected.files.iter().map(|f| f.path.clone()).collect();
        if expected_paths.len() != expected.files.len() {
            return Err(RuntimeError::ManifestMismatch);
        }
        if let Some(path) = &self.manifest {
            let metadata = fs::symlink_metadata(path).map_err(|e| io(path, e))?;
            if !metadata.is_file()
                || metadata.file_type().is_symlink()
                || metadata.len() > 1024 * 1024
            {
                return Err(RuntimeError::UnsafePath(path.clone()));
            }
            let mut actual: Manifest =
                serde_json::from_slice(&fs::read(path).map_err(|e| io(path, e))?)?;
            actual.files.sort_by(|a, b| a.path.cmp(&b.path));
            if actual != expected {
                return Err(RuntimeError::ManifestMismatch);
            }
            let mut found = BTreeSet::new();
            enumerate(&self.root, &self.root, &mut found)?;
            if found != expected_paths {
                return Err(RuntimeError::ManifestMismatch);
            }
        }
        for file in &expected.files {
            let path = regular_path(&self.root, &file.path)?;
            if self.manifest.is_some() {
                let digest = format!(
                    "{:x}",
                    Sha256::digest(fs::read(&path).map_err(|e| io(&path, e))?)
                );
                if digest != file.sha256 {
                    return Err(RuntimeError::ContentMismatch(path));
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let mode = fs::metadata(&path)
                        .map_err(|e| io(&path, e))?
                        .permissions()
                        .mode()
                        & 0o777;
                    if format!("{mode:o}") != file.mode {
                        return Err(RuntimeError::ContentMismatch(path));
                    }
                }
            }
        }
        let path = self.root.join("src/config/metadata.json");
        let metadata: Metadata =
            serde_json::from_slice(&fs::read(&path).map_err(|e| io(&path, e))?)?;
        if metadata.product.version != expected.version {
            return Err(RuntimeError::ManifestMismatch);
        }
        Ok(())
    }
}

pub(crate) fn config_bytes(path: &Path) -> Result<Vec<u8>, RuntimeError> {
    let metadata = fs::metadata(path).map_err(|e| io(path, e))?;
    if !metadata.is_file() || metadata.len() > 1024 * 1024 {
        return Err(RuntimeError::InvalidConfig);
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(nix::libc::O_NONBLOCK | nix::libc::O_NOFOLLOW);
    }
    let file = options.open(path).map_err(|e| io(path, e))?;
    if !file.metadata().map_err(|e| io(path, e))?.is_file() {
        return Err(RuntimeError::InvalidConfig);
    }
    let mut bytes = Vec::new();
    file.take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| io(path, e))?;
    if bytes.len() > 1024 * 1024
        || !serde_json::from_slice::<serde_json::Value>(&bytes)?.is_object()
    {
        return Err(RuntimeError::InvalidConfig);
    }
    Ok(bytes)
}

pub fn config_digest(path: &Path) -> Result<String, RuntimeError> {
    Ok(format!("{:x}", Sha256::digest(config_bytes(path)?)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().expect("fixture");
        let root = temp.path().join("runtime");
        fs::create_dir(&root).expect("runtime root");
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        for file in compiled_manifest().expect("manifest").files {
            let target = root.join(&file.path);
            fs::create_dir_all(target.parent().expect("parent")).expect("directory");
            fs::copy(source.join(&file.path), &target).expect("runtime file");
        }
        fs::write(
            temp.path().join("runtime-manifest.json"),
            CANONICAL_MANIFEST,
        )
        .expect("manifest file");
        (temp, root)
    }

    #[test]
    fn source_requires_a_complete_runtime_without_bootstrapping() {
        let (temp, root) = fixture();
        assert!(Runtime::source(&root).is_ok());
        fs::remove_file(root.join("config-templates/prefinalize.cmd"))
            .expect("remove fixture file");
        assert!(Runtime::source(&root).is_err());
        assert!(Runtime::resolve(None, &temp.path().join("bin/wor-flasher")).is_err());
    }

    #[test]
    fn bundled_inventory_is_anchored_in_the_executable_not_an_editable_manifest() {
        let (temp, root) = fixture();
        let manifest = temp.path().join("runtime-manifest.json");
        assert!(Runtime::verified(&root, &manifest).is_ok());
        let changed = root.join("config-templates/prefinalize.cmd");
        fs::write(&changed, "changed").expect("tamper");
        assert!(Runtime::verified(&root, &manifest).is_err());
        let mut forged: serde_json::Value = serde_json::from_str(CANONICAL_MANIFEST).expect("json");
        for entry in forged["files"].as_array_mut().expect("files") {
            if entry["path"] == "config-templates/prefinalize.cmd" {
                entry["sha256"] = format!("{:x}", Sha256::digest(b"changed")).into();
            }
        }
        fs::write(&manifest, serde_json::to_vec(&forged).expect("json")).expect("manifest");
        assert!(matches!(
            Runtime::verified(&root, &manifest),
            Err(RuntimeError::ManifestMismatch)
        ));
    }

    #[test]
    fn rejects_extra_files_duplicate_entries_and_symbolic_links() {
        let (temp, root) = fixture();
        let manifest = temp.path().join("runtime-manifest.json");
        fs::write(root.join("extra.txt"), "extra").expect("extra");
        assert!(Runtime::verified(&root, &manifest).is_err());
        fs::remove_file(root.join("extra.txt")).expect("remove");
        let mut duplicate: serde_json::Value =
            serde_json::from_str(CANONICAL_MANIFEST).expect("json");
        let first = duplicate["files"][0].clone();
        duplicate["files"]
            .as_array_mut()
            .expect("files")
            .push(first);
        fs::write(&manifest, serde_json::to_vec(&duplicate).expect("json")).expect("manifest");
        assert!(Runtime::verified(&root, &manifest).is_err());
        #[cfg(unix)]
        {
            fs::write(&manifest, CANONICAL_MANIFEST).expect("manifest");
            let path = root.join("NOTICE");
            fs::remove_file(&path).expect("remove");
            std::os::unix::fs::symlink(temp.path().join("missing"), &path).expect("symlink");
            assert!(Runtime::verified(&root, &manifest).is_err());
        }
    }

    #[test]
    fn configuration_digest_rejects_invalid_shapes_without_logging_contents() {
        let temp = tempfile::tempdir().expect("fixture");
        let path = temp.path().join("config.json");
        for invalid in ["[]", "false", "{invalid"] {
            fs::write(&path, invalid).expect("fixture");
            assert!(config_digest(&path).is_err());
        }
        fs::write(&path, r#"{"execution":{"dryRun":false}}"#).expect("fixture");
        let before = config_digest(&path).expect("valid");
        fs::write(&path, r#"{"execution":{"dryRun":true}}"#).expect("fixture");
        assert_ne!(before, config_digest(&path).expect("changed"));
    }

    #[test]
    fn macos_companion_resolves_resources_instead_of_a_mutable_working_directory() {
        let (temp, root) = fixture();
        let resources = temp.path().join("Example.app/Contents/Resources");
        fs::create_dir_all(&resources).expect("resources");
        fs::rename(root, resources.join("runtime")).expect("runtime");
        fs::rename(
            temp.path().join("runtime-manifest.json"),
            resources.join("runtime-manifest.json"),
        )
        .expect("manifest");
        assert!(
            Runtime::resolve(
                None,
                &temp.path().join("Example.app/Contents/MacOS/wor-flasher")
            )
            .is_ok()
        );
    }
}
