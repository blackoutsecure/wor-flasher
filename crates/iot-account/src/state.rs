use crate::{AccountError, Pairing, Result, Secret, validate_password};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt, symlink};
use std::path::{Component, Path, PathBuf};

fn no_follow_file(path: &Path) -> Result<File> {
    let before = std::fs::symlink_metadata(path)?;
    if !before.is_file()
        || before.file_type().is_symlink()
        || before.uid() != unsafe { libc::geteuid() }
        || before.mode() & 0o077 != 0
        || before.nlink() != 1
    {
        return Err(AccountError::invalid(
            "IoT private input must be a current-user, private, single-link regular file.",
        ));
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)?;
    let after = file.metadata()?;
    if (
        before.dev(),
        before.ino(),
        before.mode(),
        before.uid(),
        before.len(),
        before.mtime(),
        before.mtime_nsec(),
        before.ctime(),
        before.ctime_nsec(),
    ) != (
        after.dev(),
        after.ino(),
        after.mode(),
        after.uid(),
        after.len(),
        after.mtime(),
        after.mtime_nsec(),
        after.ctime(),
        after.ctime_nsec(),
    ) {
        return Err(AccountError::invalid(
            "IoT private input changed while opening.",
        ));
    }
    Ok(file)
}

fn root_path(root: &Path) -> Result<PathBuf> {
    let root = if root.is_absolute() {
        root.to_owned()
    } else {
        std::env::current_dir()?.join(root)
    };
    if root.components().any(|part| part == Component::ParentDir) || root.components().count() > 64
    {
        return Err(AccountError::invalid("Unsafe private-state path."));
    }
    let mut path = PathBuf::new();
    for part in root.components() {
        path.push(part.as_os_str());
        let metadata = std::fs::symlink_metadata(&path)?;
        if !metadata.is_dir()
            || metadata.file_type().is_symlink()
            || ![0, unsafe { libc::geteuid() }].contains(&metadata.uid())
            || (metadata.mode() & 0o022 != 0 && metadata.mode() & 0o1000 == 0)
        {
            return Err(AccountError::invalid(
                "IoT private-state directory has unsafe ownership, permissions or a symlink.",
            ));
        }
    }
    let metadata = std::fs::symlink_metadata(&root)?;
    if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o022 != 0 {
        return Err(AccountError::invalid(
            "IoT private-state root must be a current-user directory that is not group/world writable.",
        ));
    }
    Ok(root)
}

pub(crate) struct PrivateState {
    temp: Option<tempfile::TempDir>,
    pub(crate) directory: File,
    root: File,
    root_path: PathBuf,
    known: Option<String>,
    companion: Option<PathBuf>,
    pub(crate) path: PathBuf,
}

impl PrivateState {
    pub(crate) fn new(root: &Path, companion: &Path, pairing: &Pairing) -> Result<Self> {
        let root_path = root_path(root)?;
        let root = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&root_path)?;
        let temp = tempfile::Builder::new()
            .prefix(".wor-iot-account-")
            .tempdir_in(&root_path)?;
        std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700))?;
        let path = temp.path().to_owned();
        let directory = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&path)?;
        let mut state = Self {
            temp: Some(temp),
            directory,
            root,
            root_path,
            known: None,
            companion: None,
            path,
        };
        state.check()?;
        let known = format!("wor-iot-paired {} {}\n", pairing.key_type, pairing.key);
        state.write("known_hosts", known.as_bytes())?;
        state.known = Some(known);
        // OpenSSH execs this native companion directly. The root CLI routes this
        // basename to `iot-account askpass` before normal argument/runtime setup.
        let companion = if companion.is_absolute() {
            companion.to_owned()
        } else {
            std::env::current_dir()?.join(companion)
        };
        symlink(&companion, state.path.join("wor-iot-askpass"))?;
        state.companion = Some(companion);
        state.check()?;
        Ok(state)
    }

    pub(crate) fn check(&self) -> Result<()> {
        let root_metadata = std::fs::symlink_metadata(root_path(&self.root_path)?)?;
        let held_root = self.root.metadata()?;
        let current = std::fs::symlink_metadata(&self.path)?;
        let held = self.directory.metadata()?;
        if !current.is_dir()
            || current.file_type().is_symlink()
            || current.mode() & 0o077 != 0
            || current.uid() != unsafe { libc::geteuid() }
            || (current.dev(), current.ino()) != (held.dev(), held.ino())
            || (root_metadata.dev(), root_metadata.ino()) != (held_root.dev(), held_root.ino())
        {
            return Err(AccountError::invalid(
                "IoT private-state directory changed; device state is unverified.",
            ));
        }
        if let Some(expected) = &self.known {
            let mut file = no_follow_file(&self.path.join("known_hosts"))?;
            let before = file.metadata()?;
            let mut known = String::new();
            Read::by_ref(&mut file)
                .take(65537)
                .read_to_string(&mut known)?;
            let after = file.metadata()?;
            if &known != expected
                || (
                    before.len(),
                    before.mtime(),
                    before.mtime_nsec(),
                    before.ctime(),
                    before.ctime_nsec(),
                ) != (
                    after.len(),
                    after.mtime(),
                    after.mtime_nsec(),
                    after.ctime(),
                    after.ctime_nsec(),
                )
            {
                return Err(AccountError::invalid(
                    "The pinned SSH known_hosts changed; configuration is not verified.",
                ));
            }
        }
        if let Some(expected) = &self.companion
            && std::fs::read_link(self.path.join("wor-iot-askpass"))? != *expected
        {
            return Err(AccountError::invalid(
                "The native askpass companion changed; configuration is not verified.",
            ));
        }
        Ok(())
    }

    fn write(&self, name: &str, bytes: &[u8]) -> Result<()> {
        self.check()?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(self.path.join(name))?;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        file.write_all(bytes)?;
        file.sync_all()?;
        self.check()
    }

    pub(crate) fn password(&self, password: &Secret) -> Result<PathBuf> {
        self.write("current-password", password.expose().as_bytes())?;
        Ok(self.path.join("current-password"))
    }

    pub(crate) fn remove_password(&self) -> Result<()> {
        std::fs::remove_file(self.path.join("current-password"))?;
        self.check()
    }

    pub(crate) fn close(mut self) -> Result<()> {
        self.check()?;
        self.temp
            .take()
            .expect("private state owns its directory")
            .close()
            .map_err(|_| {
                AccountError::invalid(
                    "IoT temporary-state cleanup failed; configuration is not verified.",
                )
            })?;
        Ok(())
    }
}

pub fn read_askpass_file(path: &Path) -> Result<Secret> {
    let mut file = no_follow_file(path)?;
    if file.metadata()?.len() > 1024 {
        return Err(AccountError::invalid("Invalid private IoT password input."));
    }
    let before = file.metadata()?;
    let mut password = String::new();
    Read::by_ref(&mut file)
        .take(1025)
        .read_to_string(&mut password)?;
    let after = file.metadata()?;
    let named = std::fs::symlink_metadata(path)?;
    if (
        before.len(),
        before.mtime(),
        before.mtime_nsec(),
        before.ctime(),
        before.ctime_nsec(),
    ) != (
        after.len(),
        after.mtime(),
        after.mtime_nsec(),
        after.ctime(),
        after.ctime_nsec(),
    ) || after.len() > 1024
        || (
            after.dev(),
            after.ino(),
            after.len(),
            after.mtime(),
            after.mtime_nsec(),
            after.ctime(),
            after.ctime_nsec(),
            after.nlink(),
        ) != (
            named.dev(),
            named.ino(),
            named.len(),
            named.mtime(),
            named.mtime_nsec(),
            named.ctime(),
            named.ctime_nsec(),
            named.nlink(),
        )
        || named.file_type().is_symlink()
        || named.mode() & 0o077 != 0
        || named.nlink() != 1
    {
        return Err(AccountError::invalid(
            "IoT private password input changed while reading.",
        ));
    }
    validate_password(&password, false)?;
    Ok(Secret::new(password))
}
