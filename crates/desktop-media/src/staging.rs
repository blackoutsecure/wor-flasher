use crate::files::{finish_temp, read_required, require_directory, write_private};
use crate::options::absolute_path;
use crate::{MediaError, PreparationReport, PreparedMedia};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

const RECEIPT: &str = ".wor-desktop-media-owner.json";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
struct Identity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    owner: u32,
}

fn identity(path: &Path) -> Result<Identity, MediaError> {
    require_directory(path, "private staging root")?;
    let metadata = fs::symlink_metadata(path)
        .map_err(|e| MediaError::io("inspect private staging identity", e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.mode() & 0o077 != 0 {
            return Err(MediaError::InvalidMetadata("private staging permissions"));
        }
        Ok(Identity {
            device: metadata.dev(),
            inode: metadata.ino(),
            owner: metadata.uid(),
        })
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        Ok(Identity {})
    }
}

/// Explicitly retained preparation output, never a device image publication.
/// Its receipt binds cleanup to the original private directory identity.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StagedMedia {
    pub staging_dir: PathBuf,
    pub boot_dir: PathBuf,
    pub install_wim: PathBuf,
    pub install_extras: PathBuf,
    pub build: String,
    pub language: String,
    pub report: PreparationReport,
    schema: u32,
    identity: Identity,
}

impl StagedMedia {
    pub fn load(directory: &Path) -> Result<Self, MediaError> {
        let directory = absolute_path(directory)?;
        let bytes = read_required(
            &directory.join(RECEIPT),
            "native preparation ownership receipt",
            4 * 1024 * 1024,
        )?;
        let receipt: Self = serde_json::from_slice(&bytes)
            .map_err(|_| MediaError::InvalidMetadata("native preparation ownership receipt"))?;
        receipt.validate(&directory)?;
        Ok(receipt)
    }

    fn validate(&self, directory: &Path) -> Result<(), MediaError> {
        if self.schema != 1
            || identity(directory)? != self.identity
            || directory
                .canonicalize()
                .map_err(|e| MediaError::io("resolve staging identity", e))?
                != self.staging_dir
            || self.boot_dir != self.staging_dir.join("boot")
            || self.install_extras != self.staging_dir.join("install-extras")
            || self.install_wim != self.staging_dir.join("install.wim")
                && self.install_wim != self.staging_dir.join("pi3-normalized/install.wim")
        {
            return Err(MediaError::InvalidMetadata(
                "native preparation ownership binding",
            ));
        }
        Ok(())
    }

    pub fn cleanup(self) -> Result<(), MediaError> {
        self.validate(&self.staging_dir)?;
        let current = Self::load(&self.staging_dir)?;
        if current.identity != self.identity {
            return Err(MediaError::InvalidMetadata(
                "changed native preparation ownership",
            ));
        }
        fs::remove_dir_all(&self.staging_dir).map_err(|source| MediaError::Cleanup {
            primary: None,
            source,
        })
    }
}

pub(crate) fn persist(
    mut media: PreparedMedia,
    destination: &Path,
) -> Result<StagedMedia, MediaError> {
    let staging = media.staging.take().expect("owned preparation lifetime");
    let old = staging.path().to_path_buf();
    let destination = absolute_path(destination);
    let result = (|| {
        let destination = destination?;
        let parent = destination
            .parent()
            .ok_or(MediaError::InvalidMetadata("retained staging destination"))?;
        require_directory(parent, "retained staging parent")?;
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        // Claim a NEW directory atomically. rename must never replace a user's
        // pre-existing empty directory, let alone an existing prepared output.
        builder
            .create(&destination)
            .map_err(|e| MediaError::io("claim new private preparation output", e))?;
        if let Err(error) = fs::rename(&old, &destination) {
            if let Err(source) = fs::remove_dir(&destination) {
                return Err(MediaError::Cleanup {
                    primary: Some(Box::new(MediaError::io(
                        "retain private preparation output on the same filesystem",
                        error,
                    ))),
                    source,
                });
            }
            return Err(MediaError::io(
                "retain private preparation output on the same filesystem",
                error,
            ));
        }
        let result = (|| {
            let destination = destination
                .canonicalize()
                .map_err(|e| MediaError::io("resolve retained preparation output", e))?;
            let relative_image = media
                .install_wim
                .strip_prefix(&old)
                .map_err(|_| MediaError::InvalidMetadata("owned installation-image path"))?;
            let staged = StagedMedia {
                schema: 1,
                identity: identity(&destination)?,
                boot_dir: destination.join("boot"),
                install_wim: destination.join(relative_image),
                install_extras: destination.join("install-extras"),
                staging_dir: destination,
                build: media.build.clone(),
                language: media.language.clone(),
                report: media.report.clone(),
            };
            let bytes = serde_json::to_vec(&staged)
                .map_err(|_| MediaError::InvalidMetadata("retained preparation receipt"))?;
            write_private(&staged.staging_dir.join(RECEIPT), &bytes)?;
            Ok(staged)
        })();
        if let Err(error) = result {
            if let Err(source) = fs::remove_dir_all(&destination) {
                return Err(MediaError::Cleanup {
                    primary: Some(Box::new(error)),
                    source,
                });
            }
            return Err(error);
        }
        result
    })();
    if !old.exists() {
        // Ownership moved with the directory. Closing the old TempDir path
        // would incorrectly report ENOENT or target a subsequently reused path.
        let _old_name = staging.keep();
        result
    } else {
        finish_temp(staging, result)
    }
}
