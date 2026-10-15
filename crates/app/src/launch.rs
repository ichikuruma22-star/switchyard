use crate::{AppError, Result};
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// Private host credentials. Intentionally has no Debug implementation.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchInfo {
    pub schema_version: u32,
    pub url: String,
    pub base_url: String,
    pub token: String,
    pub pid: u32,
}

/// The single database owner advertises its listener in a private, protected
/// directory. A stable path lets MCP clients reconnect after a host restart.
pub(crate) struct HostRecord {
    path: PathBuf,
    token: String,
}
impl HostRecord {
    pub(crate) fn path(data: &Path) -> PathBuf {
        data.join(".switchya").join("host.json")
    }
    pub(crate) fn publish(info: &LaunchInfo, data: &Path) -> Result<Self> {
        let directory = data.join(".switchya");
        crate::startup::private_directory(&directory)?;
        let path = checked_path(&Self::path(data))?;
        if path
            .try_exists()
            .map_err(|_| AppError::local("Could not inspect the active-host record."))?
        {
            LaunchInfo::read(&path)?;
        }
        let temporary = directory.join(format!("host-{}.json", uuid::Uuid::new_v4()));
        info.write_new(&temporary)?;
        if std::fs::rename(&temporary, &path).is_err() {
            let _ = std::fs::remove_file(&temporary);
            return Err(AppError::local(
                "Could not publish the private active-host record.",
            ));
        }
        Ok(Self {
            path,
            token: info.token.clone(),
        })
    }
}
impl Drop for HostRecord {
    fn drop(&mut self) {
        if LaunchInfo::read(&self.path).is_ok_and(|current| current.token == self.token) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

impl LaunchInfo {
    pub(crate) fn new(port: u16) -> Self {
        let token: String = rand::random::<[u8; 32]>()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let base_url = format!("http://127.0.0.1:{port}");
        Self {
            schema_version: 1,
            url: format!("{base_url}/#token={token}"),
            base_url,
            token,
            pid: std::process::id(),
        }
    }

    pub(crate) fn validate(&self) -> Result<()> {
        let bad = || {
            AppError::invalid(
                "Launch information must contain an exact loopback HTTP origin and its matching host-session token.",
            )
        };
        let parsed = url::Url::parse(&self.base_url).map_err(|_| bad())?;
        let Some(port) = parsed.port_or_known_default().filter(|port| *port != 0) else {
            return Err(bad());
        };
        if self.schema_version != 1
            || self.pid == 0
            || self.base_url != format!("http://127.0.0.1:{port}")
            || self.token.len() != 64
            || !self.token.bytes().all(|b| b.is_ascii_hexdigit())
            || self.url != format!("{}/#token={}", self.base_url, self.token)
        {
            return Err(bad());
        }
        Ok(())
    }

    /// Writes only to a new file in an existing directory, with private file access.
    /// Unix files are mode 0600. Windows uses a protected current-user/SYSTEM DACL.
    pub fn write_new(&self, path: impl AsRef<Path>) -> Result<()> {
        self.validate()?;
        let path = checked_path(path.as_ref())?;
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options
                .mode(0o600)
                .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            use windows_sys::Win32::Foundation::GENERIC_WRITE;
            use windows_sys::Win32::Storage::FileSystem::{READ_CONTROL, WRITE_DAC, WRITE_OWNER};
            // Exclusive sharing prevents a reader acquiring the initially empty
            // file before its inherited ACL has been replaced.
            options
                .access_mode(GENERIC_WRITE | READ_CONTROL | WRITE_DAC | WRITE_OWNER)
                .share_mode(0);
        }
        let mut file = options.open(&path).map_err(|_| {
            AppError::invalid(
                "Launch-info output must be a new file in an existing directory with access-control support.",
            )
        })?;
        let prepared = (|| {
            if !file
                .metadata()
                .map_err(|_| AppError::local("Could not inspect the launch-info file."))?
                .is_file()
            {
                return Err(AppError::invalid(
                    "Launch information requires a regular file.",
                ));
            }
            #[cfg(windows)]
            crate::launch_acl::restrict(&file)?;
            Ok(())
        })();
        let written = prepared.and_then(|()| {
            serde_json::to_vec(self)
                .map_err(|_| AppError::local("Could not encode launch information."))
                .and_then(|bytes| {
                    file.write_all(&bytes)
                        .and_then(|()| file.sync_all())
                        .map_err(|_| AppError::local("Could not write launch information."))
                })
        });
        drop(file);
        if written.is_err() {
            let _ = std::fs::remove_file(path);
        }
        written
    }

    /// Reads an explicitly supplied private file; never follows a launch-file symlink.
    pub fn read(path: impl AsRef<Path>) -> Result<Self> {
        let path = checked_path(path.as_ref())?;
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            use windows_sys::Win32::Storage::FileSystem::{
                FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
            };
            options
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
                .share_mode(FILE_SHARE_READ);
        }
        let file = options
            .open(path)
            .map_err(|_| AppError::invalid("Could not read the launch-info file."))?;
        check_opened_file(&file)?;
        let mut bytes = Vec::new();
        file.take(16_385)
            .read_to_end(&mut bytes)
            .map_err(|_| AppError::invalid("Could not read the launch-info file."))?;
        if bytes.len() > 16_384 {
            return Err(AppError::invalid("Launch information is too large."));
        }
        let info: Self = serde_json::from_slice(&bytes)
            .map_err(|_| AppError::invalid("The launch-info file has an invalid format."))?;
        info.validate()?;
        Ok(info)
    }
}

fn check_opened_file(file: &File) -> Result<()> {
    let metadata = file
        .metadata()
        .map_err(|_| AppError::invalid("Could not inspect the opened launch-info file."))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 16_384 {
        return Err(AppError::invalid(
            "Launch information must be a small regular file.",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if metadata.permissions().mode() & 0o077 != 0
            || metadata.uid() != rustix::process::geteuid().as_raw()
        {
            return Err(AppError::invalid(
                "Launch information must be owned by this user and readable/writable only by its owner.",
            ));
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(AppError::invalid(
                "Launch information cannot be a reparse point.",
            ));
        }
        crate::launch_acl::verify(file)?;
    }
    Ok(())
}

fn checked_path(path: &Path) -> Result<PathBuf> {
    let name = path
        .file_name()
        .ok_or_else(|| AppError::invalid("Supply an explicit launch-info filename."))?;
    #[cfg(windows)]
    if name.to_string_lossy().contains(':') {
        return Err(AppError::invalid(
            "Launch information cannot use an alternate data stream.",
        ));
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let parent = std::fs::canonicalize(parent)
        .map_err(|_| AppError::invalid("The launch-info parent directory must already exist."))?;
    let metadata = parent
        .metadata()
        .map_err(|_| AppError::invalid("Could not inspect the launch-info directory."))?;
    if !metadata.is_dir() {
        return Err(AppError::invalid(
            "The launch-info parent must be a directory.",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o022 != 0 {
            return Err(AppError::invalid(
                "The launch-info directory must not be writable by other users.",
            ));
        }
    }
    Ok(parent.join(name))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn private_host_record_rotates_without_an_old_owner_removing_a_new_record() {
        let temp = tempfile::tempdir().unwrap();
        let first = HostRecord::publish(&LaunchInfo::new(18317), temp.path()).unwrap();
        let next = LaunchInfo::new(18318);
        let second = HostRecord::publish(&next, temp.path()).unwrap();
        drop(first);
        let path = HostRecord::path(temp.path());
        assert_eq!(LaunchInfo::read(&path).unwrap().token, next.token);
        drop(second);
        assert!(!path.exists());
        std::fs::write(&path, "An unrelated file must not be overwritten.").unwrap();
        assert!(HostRecord::publish(&next, temp.path()).is_err());
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            "An unrelated file must not be overwritten."
        );
    }
    #[test]
    fn launch_info_round_trips_without_overwrite_and_rejects_remote_urls() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("launch.json");
        let info = LaunchInfo::new(18317);
        LaunchInfo::new(80).validate().unwrap();
        info.write_new(&path).unwrap();
        assert_eq!(LaunchInfo::read(&path).unwrap().url, info.url);
        assert!(info.write_new(&path).is_err());
        let mut bad = info.clone();
        bad.base_url = "https://example.invalid:18317".into();
        assert!(bad.validate().is_err());
        bad = info.clone();
        bad.url = format!("{}/?token={}", bad.base_url, bad.token);
        assert!(bad.validate().is_err());
        bad = info;
        bad.token.push('a');
        assert!(bad.validate().is_err());
    }

    #[cfg(unix)]
    #[test]
    fn launch_info_rejects_symlinks_and_shared_mode_on_opened_file() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("launch.json");
        LaunchInfo::new(18317).write_new(&target).unwrap();
        let link = temp.path().join("link.json");
        symlink(&target, &link).unwrap();
        assert!(LaunchInfo::read(&link).is_err());
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(LaunchInfo::read(&target).is_err());
    }
}
