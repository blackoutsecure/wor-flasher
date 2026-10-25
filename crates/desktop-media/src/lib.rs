//! Native, device-independent Windows ARM64 media preparation.
//!
//! The returned startup tree is ready to copy, not a request for a writer to run
//! another image-customization engine. Downloaded base caches never contain a
//! run's account credentials or boot preferences.

mod archive;
mod cache;
mod catalog;
mod cli;
mod files;
mod locale;
mod options;
mod pipeline;
mod provider;
mod setup;
mod staging;
mod wim;
mod xml;

pub use archive::{ArchiveLimits, extract_zip};
pub use cache::{cache_is_current, cache_manifest, mark_cache};
pub use catalog::{
    BuildRelease, CachedWindowsMedia, CatalogueEntry, Language, QueryOptions, ReleaseKind,
    ReleaseSelection, ReleaseVersion, cached_windows_media, catalog, catalog_with_provider,
    default_language, get_esd_catalog_entry, languages, locales_from_xml, parse_build_releases,
    parse_release_page, query_builds, query_builds_with_provider, query_catalogue,
    query_catalogue_with_provider, query_release_versions, query_release_versions_with_provider,
    windows_locale,
};
pub use cli::{run, run_with_io};
pub use locale::{query_locale_options, query_locale_options_with_provider};
pub use options::{
    BootOptions, BuildId, CacheMode, DesktopOptions, InstallationMode, Model, PackagePolicy,
    PrepareOptions, Secret, SetupOptions, SystemDefaults, WindowsSource, WindowsVersion,
};
pub use pipeline::{PI4_REQUIRED_DRIVERS, prepare, prepare_typed, prepare_with_provider};
pub use provider::{
    DownloadRequest, MediaProvider, MountStyle, NativeProvider, NativeTools, WimOperation,
};
pub use setup::answer_file;
pub use staging::StagedMedia;
pub use wim::{
    ImageMetadata, ResourceSummary, WimInfo, parse_image_metadata, parse_resource_summary,
    parse_wim_info,
};

use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use tempfile::TempDir;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum MediaError {
    #[error("Invalid desktop preparation option {key}: {reason}")]
    InvalidOption {
        key: &'static str,
        reason: &'static str,
    },
    #[error("Missing or unsafe required {component}: {path}")]
    MissingRequired {
        component: &'static str,
        path: PathBuf,
    },
    #[error("Invalid {0} metadata")]
    InvalidMetadata(&'static str),
    #[error("{component} integrity verification failed")]
    Integrity { component: &'static str },
    #[error("Could not download {component}: {reason}")]
    Download {
        component: &'static str,
        reason: &'static str,
    },
    #[error("{tool} could not complete {operation} (exit status {status:?})")]
    Tool {
        tool: &'static str,
        operation: &'static str,
        status: Option<i32>,
    },
    #[error("Could not {operation}: {source}")]
    Io {
        operation: &'static str,
        #[source]
        source: io::Error,
    },
    #[error("Not enough free staging space: {required} bytes needed, {available} available")]
    Space { required: u64, available: u64 },
    #[error("Desktop media preparation cancelled")]
    Cancelled,
    #[error("Could not clean up private preparation state after {primary:?}: {source}")]
    Cleanup {
        primary: Option<Box<MediaError>>,
        #[source]
        source: io::Error,
    },
}

impl MediaError {
    pub(crate) fn io(operation: &'static str, source: io::Error) -> Self {
        Self::Io { operation, source }
    }
}

pub(crate) fn check_cancelled(cancelled: &AtomicBool) -> Result<(), MediaError> {
    if cancelled.load(Ordering::Acquire) {
        Err(MediaError::Cancelled)
    } else {
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PreparationWarning {
    pub code: String,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PackageReport {
    pub kind: String,
    pub version: String,
    pub origin: String,
    pub cache_reused: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PreparationReport {
    pub model: Model,
    pub build: String,
    pub language: String,
    pub source: String,
    pub cache_mode: CacheMode,
    pub packages: Vec<PackageReport>,
    pub warnings: Vec<PreparationWarning>,
}

/// Owns the lifetime of all per-run files, including credential-bearing answers.
///
/// Writers must call `cleanup` and propagate its error before reporting verified
/// success. Drop remains a best-effort backstop for callers unwinding or aborting.
#[derive(Debug)]
pub struct PreparedMedia {
    pub boot_dir: PathBuf,
    pub install_wim: PathBuf,
    pub install_extras: PathBuf,
    pub build: String,
    pub language: String,
    pub report: PreparationReport,
    staging: Option<TempDir>,
}

impl PreparedMedia {
    pub fn staging_dir(&self) -> &Path {
        self.boot_dir.parent().expect("owned preparation tree")
    }

    pub fn cleanup(mut self) -> Result<(), MediaError> {
        if let Some(staging) = self.staging.take() {
            staging.close().map_err(|source| MediaError::Cleanup {
                primary: None,
                source,
            })?;
        }
        Ok(())
    }

    /// Explicit helper-command retention only. Normal writers should keep the
    /// RAII lifetime and call cleanup rather than retain credential-bearing data.
    pub fn persist_to(self, destination: &Path) -> Result<StagedMedia, MediaError> {
        staging::persist(self, destination)
    }
}
