use super::{
    ChangedFile, EditArgs, MAX_FILE_BYTES, MAX_PATCH_BYTES, ReadArgs, SearchArgs, ToolError,
    ToolResult, sha256,
};
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};
use tokio_util::sync::CancellationToken;

const MAX_SEARCH_FILES: usize = 2_000;
const MAX_SEARCH_ENTRIES: usize = 8_000;
const MAX_SEARCH_BYTES: usize = 8 * 1024 * 1024;
const MAX_SEARCH_DEPTH: usize = 24;

fn invalid(message: impl Into<String>) -> ToolError {
    ToolError::Invalid(message.into())
}

fn blocked_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    // Conventional templates are useful project input, while actual local
    // environment files remain excluded. This is a filename policy, not a
    // content classification guarantee.
    if matches!(n.as_str(), ".env.example" | ".env.sample" | ".env.template") {
        return false;
    }
    matches!(
        n.as_str(),
        ".git"
            | ".hg"
            | ".svn"
            | ".ssh"
            | ".aws"
            | ".azure"
            | ".codex"
            | ".claude"
            | ".switchya"
            | ".switchyard"
            | "auth.json"
            | "auth-store.json"
            | "tokens.json"
            | "switchyard.toml"
            | ".npmrc"
            | ".pypirc"
            | ".netrc"
            | "credentials"
            | "credentials.json"
            | "secrets"
            | "secrets.json"
            | "secrets.yaml"
            | "secrets.yml"
            | "secrets.toml"
            | "id_rsa"
            | "id_dsa"
            | "id_ecdsa"
            | "id_ed25519"
    ) || n == ".env"
        || n.starts_with(".env.")
        || n.ends_with(".pem")
        || n.ends_with(".key")
        || n.ends_with(".p12")
        || n.ends_with(".pfx")
        || n.starts_with("credentials.")
        || n.starts_with("secret.")
        || n.starts_with("secrets.")
        || n.starts_with("sessions.sqlite3")
}

fn ignored_directory(name: &str) -> bool {
    blocked_name(name)
        || matches!(
            name.to_ascii_lowercase().as_str(),
            "node_modules"
                | "target"
                | "vendor"
                | "dist"
                | "build"
                | ".next"
                | ".cache"
                | ".venv"
                | "venv"
                | "__pycache__"
        )
}

/// Files other programs load and act on without being asked: shell startup
/// files, PowerShell profiles, Git configuration and hook managers, and editor
/// and agent settings. Reading them and reviewed edits are allowed; edits no
/// person reviews are not. A short list of well-known names, not a guarantee.
fn auto_loaded_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    matches!(
        n.as_str(),
        ".bashrc"
            | ".bash_profile"
            | ".bash_login"
            | ".bash_logout"
            | ".profile"
            | ".zshrc"
            | ".zshenv"
            | ".zprofile"
            | ".zlogin"
            | ".zlogout"
            | ".cshrc"
            | ".tcshrc"
            | ".kshrc"
            | ".envrc"
            | ".gitconfig"
            | ".githooks"
            | ".husky"
            | ".pre-commit-config.yaml"
            | ".vscode"
            | ".mcp.json"
            | ".claude.json"
    ) || n.ends_with("profile.ps1")
}

pub(super) fn check_unreviewed_edit(root: &Path, relative: &str) -> Result<(), ToolError> {
    let parts = relative_parts(relative, false)?;
    if parts
        .iter()
        .any(|part| auto_loaded_name(&part.to_string_lossy()))
    {
        return Err(invalid(
            "files that other programs load or run automatically need a person's review",
        ));
    }
    if contains_home(root) {
        return Err(invalid(
            "every edit in a project that contains the home folder needs a person's review",
        ));
    }
    match resolve(root, relative, false, false) {
        Err(ToolError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        result => result.map(|_| ()),
    }
}

/// Whether `root` is the user's home folder or one of its parents.
fn contains_home(root: &Path) -> bool {
    let Some(home) = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
    else {
        return false;
    };
    let home = home.canonicalize().unwrap_or(home);
    super::path_within(&home, root)
}

fn is_link(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // Includes junctions and other reparse points, not only symbolic links.
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn relative_parts(relative: &str, allow_root: bool) -> Result<Vec<&std::ffi::OsStr>, ToolError> {
    if relative.is_empty()
        || relative.contains('\0')
        || relative.contains(':')
        || relative.len() > 4096
    {
        return Err(invalid(
            "path must be a nonempty project-relative path without NUL or ':'",
        ));
    }
    let mut parts = Vec::new();
    for component in Path::new(relative).components() {
        match component {
            Component::Normal(name) => {
                let name_text = name.to_string_lossy();
                if blocked_name(&name_text) {
                    return Err(invalid(
                        "repository metadata and likely secret files are blocked",
                    ));
                }
                // Windows strips trailing spaces/dots and interprets device names.
                // Reject these consistently on every platform.
                let stem = name_text
                    .split('.')
                    .next()
                    .unwrap_or("")
                    .to_ascii_uppercase();
                if name_text.ends_with([' ', '.'])
                    || matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
                    || (stem.len() == 4
                        && (stem.starts_with("COM") || stem.starts_with("LPT"))
                        && matches!(stem.as_bytes()[3], b'1'..=b'9'))
                {
                    return Err(invalid("ambiguous or reserved path component"));
                }
                parts.push(name);
            }
            Component::CurDir => {}
            _ => {
                return Err(invalid(
                    "absolute paths and parent traversal are not allowed",
                ));
            }
        }
    }
    if parts.is_empty() && !allow_root {
        return Err(invalid("a file path is required"));
    }
    Ok(parts)
}

pub(super) fn check_relative(relative: &str) -> Result<(), ToolError> {
    relative_parts(relative, false).map(|_| ())
}

/// Hold the complete path on Windows; walk directory descriptors on Unix.
/// Listing uses the opened directory, so replacing a parent with a link cannot
/// redirect the browser into another part of the filesystem.
pub(super) fn directory(root: &Path, relative: &str) -> Result<Value, ToolError> {
    const MAX_ENTRIES: usize = 2_000;
    let _path = resolve(root, relative, true, false)?;
    let parts = relative_parts(relative, true)?;
    let mut directory = open_directory(root)?;
    #[cfg(windows)]
    let mut held = Vec::new();
    #[cfg(windows)]
    let mut current = root.to_path_buf();
    for part in parts {
        #[cfg(unix)]
        {
            directory = unix_open_at(
                &directory,
                part,
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0,
            )?;
        }
        #[cfg(windows)]
        {
            current.push(part);
            let next = open_directory(&current)?;
            held.push(directory);
            directory = next;
        }
    }
    let mut entries = Vec::new();
    let mut omitted = 0;
    let mut truncated = false;
    #[cfg(unix)]
    let names = rustix::fs::Dir::read_from(&directory)
        .map_err(std::io::Error::from)?
        .map(|entry| entry.map_err(std::io::Error::from));
    #[cfg(windows)]
    let names = fs::read_dir(&_path)?;
    for (index, entry) in names.enumerate() {
        if index >= MAX_ENTRIES {
            truncated = true;
            break;
        }
        let entry = entry?;
        #[cfg(unix)]
        let name = match entry.file_name().to_str() {
            Ok(name) if !matches!(name, "." | "..") => name.to_owned(),
            _ => continue,
        };
        #[cfg(windows)]
        let name = match entry.file_name().into_string() {
            Ok(name) => name,
            Err(_) => {
                omitted += 1;
                continue;
            }
        };
        let child_path = if relative == "." {
            name.clone()
        } else {
            format!("{}/{name}", relative.trim_end_matches('/'))
        };
        if check_relative(&child_path).is_err() {
            omitted += 1;
            continue;
        }
        #[cfg(unix)]
        let (is_file, is_dir, bytes) = {
            let stat = match rustix::fs::statat(
                &directory,
                name.as_str(),
                rustix::fs::AtFlags::SYMLINK_NOFOLLOW,
            ) {
                Ok(stat) => stat,
                Err(_) => {
                    omitted += 1;
                    continue;
                }
            };
            let kind = rustix::fs::FileType::from_raw_mode(stat.st_mode);
            (
                kind == rustix::fs::FileType::RegularFile,
                kind == rustix::fs::FileType::Directory,
                stat.st_size.max(0) as u64,
            )
        };
        #[cfg(windows)]
        let (is_file, is_dir, bytes) = {
            let metadata = match fs::symlink_metadata(_path.join(&name)) {
                Ok(metadata) => metadata,
                Err(_) => {
                    omitted += 1;
                    continue;
                }
            };
            if is_link(&metadata) {
                omitted += 1;
                continue;
            }
            (metadata.is_file(), metadata.is_dir(), metadata.len())
        };
        if (!is_file && !is_dir) || (is_dir && ignored_directory(&name)) {
            omitted += 1;
            continue;
        }
        entries.push(json!({"name":name,"path":child_path,"kind":if is_dir {"directory"} else {"file"},"bytes":if is_file {Some(bytes)} else {None}}));
    }
    entries.sort_by_key(|entry| {
        (
            entry["kind"] != "directory",
            entry["name"].as_str().unwrap_or("").to_lowercase(),
        )
    });
    Ok(json!({"path":relative,"entries":entries,"omitted":omitted,"truncated":truncated}))
}

/// NTFS also opens an entry by its 8.3 short name, such as `GIT~1` for `.git`,
/// which the spelling-based name policy cannot see. Check the long name stored
/// in the parent directory, and refuse aliases so previews show the real name.
#[cfg(windows)]
fn check_stored_name(path: &Path, requested: &std::ffi::OsStr) -> Result<(), ToolError> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use windows_sys::Win32::{
        Foundation::INVALID_HANDLE_VALUE,
        Storage::FileSystem::{FindClose, FindFirstFileW, WIN32_FIND_DATAW},
    };
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let mut data = WIN32_FIND_DATAW::default();
    // SAFETY: the path is NUL-terminated and data is a valid output buffer.
    let search = unsafe { FindFirstFileW(wide.as_ptr(), &mut data) };
    if search == INVALID_HANDLE_VALUE {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: the handle came from a successful FindFirstFileW call.
    unsafe { FindClose(search) };
    let length = data
        .cFileName
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(data.cFileName.len());
    let stored = std::ffi::OsString::from_wide(&data.cFileName[..length]);
    let stored = stored.to_string_lossy();
    if blocked_name(&stored) {
        return Err(invalid(
            "repository metadata and likely secret files are blocked",
        ));
    }
    // NTFS compares names through an uppercase table, so a case variant of the
    // stored name is the same entry rather than an alias.
    if stored.to_uppercase() != requested.to_string_lossy().to_uppercase() {
        return Err(invalid(
            "short (8.3) file names are not allowed; use the full name",
        ));
    }
    Ok(())
}

/// Validate every component. Links/reparse points are refused even if they point
/// back into the project. Missing final components are allowed only for creation.
pub(super) fn resolve(
    root: &Path,
    relative: &str,
    allow_root: bool,
    allow_missing: bool,
) -> Result<PathBuf, ToolError> {
    let parts = relative_parts(relative, allow_root)?;
    let root_meta = fs::symlink_metadata(root)?;
    if is_link(&root_meta) || !root_meta.is_dir() {
        return Err(invalid("project root is no longer a regular directory"));
    }
    let mut path = root.to_owned();
    let count = parts.len();
    for (index, part) in parts.into_iter().enumerate() {
        path.push(part);
        match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                if is_link(&metadata) {
                    return Err(invalid("symbolic links and reparse points are not allowed"));
                }
                #[cfg(windows)]
                check_stored_name(&path, part)?;
                if index + 1 < count && !metadata.is_dir() {
                    return Err(invalid("path parent is not a directory"));
                }
                if index + 1 == count && !(metadata.is_file() || metadata.is_dir()) {
                    return Err(invalid("only regular files and directories are supported"));
                }
            }
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound
                    && allow_missing
                    && index + 1 == count => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(path)
}

// Hold parent directory handles through file operations. On Unix, file opens and
// renames are relative to the verified directory descriptor. On Windows, handles
// deny directory deletion/renaming while the operation is in flight.
struct Anchor {
    path: PathBuf,
    #[cfg_attr(windows, allow(dead_code))] // Holds the directory against rename/deletion.
    parent: File,
    #[cfg(unix)]
    name: std::ffi::OsString,
    #[cfg(windows)]
    _parents: Vec<File>,
}

fn open_directory(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
            FILE_SHARE_WRITE,
        };
        options
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE);
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if is_link(&metadata) || !metadata.is_dir() {
        return Err(std::io::Error::other("directory changed or became a link"));
    }
    Ok(file)
}

impl Anchor {
    fn new(root: &Path, relative: &str, allow_missing: bool) -> Result<Self, ToolError> {
        let path = resolve(root, relative, false, allow_missing)?;
        let parts = relative_parts(relative, false)?;
        #[cfg(unix)]
        let name = parts.last().expect("file name validated").to_os_string();
        let mut parent = open_directory(root)?;
        #[cfg(windows)]
        let mut parents = vec![];
        #[cfg(windows)]
        let mut current = root.to_path_buf();
        for part in &parts[..parts.len() - 1] {
            #[cfg(unix)]
            {
                parent = unix_open_at(
                    &parent,
                    part,
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                    0,
                )?;
            }
            #[cfg(windows)]
            {
                current.push(part);
                let next = open_directory(&current)?;
                parents.push(parent);
                parent = next;
            }
        }
        Ok(Self {
            path,
            parent,
            #[cfg(unix)]
            name,
            #[cfg(windows)]
            _parents: parents,
        })
    }

    fn open_read(&self) -> Result<File, ToolError> {
        #[cfg(unix)]
        let file = unix_open_at(
            &self.parent,
            &self.name,
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
            0,
        )?;
        #[cfg(windows)]
        let file = {
            use std::os::windows::fs::OpenOptionsExt;
            use windows_sys::Win32::Storage::FileSystem::{
                FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
            };
            OpenOptions::new()
                .read(true)
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
                .share_mode(FILE_SHARE_READ)
                .open(&self.path)?
        };
        let metadata = file.metadata()?;
        if is_link(&metadata) || !metadata.is_file() {
            return Err(invalid("only regular files are supported"));
        }
        Ok(file)
    }

    fn read_all(&self, limit: usize) -> Result<Vec<u8>, ToolError> {
        let file = self.open_read()?;
        if file.metadata()?.len() > limit as u64 {
            return Err(invalid(format!("file exceeds the {limit}-byte edit limit")));
        }
        let mut bytes = Vec::new();
        file.take((limit + 1) as u64).read_to_end(&mut bytes)?;
        if bytes.len() > limit {
            return Err(invalid("file grew beyond the edit limit"));
        }
        Ok(bytes)
    }

    fn read_optional(&self) -> Result<Option<Vec<u8>>, ToolError> {
        match self.read_all(MAX_FILE_BYTES) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(ToolError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }
}

#[cfg(unix)]
fn unix_open_at(
    parent: &File,
    name: &std::ffi::OsStr,
    flags: i32,
    mode: libc::mode_t,
) -> std::io::Result<File> {
    use std::os::{
        fd::{AsRawFd, FromRawFd},
        unix::ffi::OsStrExt,
    };
    let name = std::ffi::CString::new(name.as_bytes())
        .map_err(|_| std::io::Error::other("NUL in file name"))?;
    // SAFETY: parent is a live directory descriptor; the C string is NUL-terminated.
    // C variadic arguments require integer promotion (macOS mode_t is u16).
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            flags,
            mode as libc::c_uint,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: successful openat returned a new descriptor exclusively owned here.
    Ok(unsafe { File::from_raw_fd(fd) })
}

/// Decode bytes read up to a limit. A read that stopped at the limit may end
/// inside a multi-byte character; only that incomplete tail is dropped.
fn utf8_text(bytes: &[u8], truncated: bool) -> Option<&str> {
    match std::str::from_utf8(bytes) {
        Ok(text) => Some(text),
        Err(error) if truncated && error.error_len().is_none() => {
            Some(std::str::from_utf8(&bytes[..error.valid_up_to()]).expect("valid UTF-8 prefix"))
        }
        Err(_) => None,
    }
}

pub(super) fn read(root: &Path, args: &ReadArgs, cancel: &CancellationToken) -> ToolResult {
    if cancel.is_cancelled() {
        return ToolResult::cancelled();
    }
    let run = || -> Result<ToolResult, ToolError> {
        let anchor = Anchor::new(root, &args.path, false)?;
        let file = anchor.open_read()?;
        let mut bytes = Vec::new();
        file.take((args.max_bytes + 1) as u64)
            .read_to_end(&mut bytes)?;
        let truncated = bytes.len() > args.max_bytes;
        if truncated {
            bytes.truncate(args.max_bytes);
        }
        let digest = (!truncated).then(|| sha256(&bytes));
        let content =
            utf8_text(&bytes, truncated).ok_or_else(|| invalid("file is not valid UTF-8 text"))?;
        if content.contains('\0') {
            return Err(invalid("binary files are not supported"));
        }
        let mut result = ToolResult::completed(
            json!({"path":args.path,"content":content,"sha256":digest,"before_sha256":digest,"truncated":truncated}),
        );
        result.truncated = truncated;
        Ok(result)
    };
    match run() {
        Ok(result) => result,
        Err(error) => ToolResult::failed(error),
    }
}

pub(super) fn search(root: &Path, args: &SearchArgs, cancel: &CancellationToken) -> ToolResult {
    let run = || -> Result<ToolResult, ToolError> {
        let start = resolve(root, &args.path, true, false)?;
        let mut stack = vec![(start, 0usize)];
        let mut matches = Vec::new();
        let mut files_searched = 0usize;
        let mut entries_seen = 0usize;
        let mut bytes_read = 0usize;
        // Entries skipped because they could not be validated or read. Only the
        // requested directory itself is a hard error.
        let mut omitted = 0usize;
        let mut truncated = false;
        'directories: while let Some((directory, depth)) = stack.pop() {
            if cancel.is_cancelled() {
                return Ok(ToolResult::cancelled());
            }
            let relative_dir = directory
                .strip_prefix(root)
                .map_err(|_| invalid("search escaped the project"))?;
            let dir_text = if relative_dir.as_os_str().is_empty() {
                ".".to_owned()
            } else {
                relative_dir.to_string_lossy().into_owned()
            };
            let listing = resolve(root, &dir_text, true, false)
                .and_then(|_| fs::read_dir(&directory).map_err(ToolError::from));
            let listing = match listing {
                Ok(listing) => listing,
                Err(error) if depth == 0 => return Err(error),
                Err(_) => {
                    omitted += 1;
                    continue;
                }
            };
            // The entry metadata is not trusted: each file is reopened through
            // Anchor before its contents are read.
            let mut entries = Vec::new();
            for entry in listing {
                if cancel.is_cancelled() {
                    return Ok(ToolResult::cancelled());
                }
                entries_seen += 1;
                if entries_seen > MAX_SEARCH_ENTRIES {
                    truncated = true;
                    break 'directories;
                }
                match entry {
                    Ok(entry) => entries.push(entry),
                    Err(_) => omitted += 1,
                }
            }
            entries.sort_by_key(|entry| entry.file_name());
            for entry in entries {
                if cancel.is_cancelled() {
                    return Ok(ToolResult::cancelled());
                }
                let Ok(name) = entry.file_name().into_string() else {
                    omitted += 1;
                    continue;
                };
                if blocked_name(&name) {
                    continue;
                }
                let Ok(metadata) = fs::symlink_metadata(entry.path()) else {
                    omitted += 1;
                    continue;
                };
                if is_link(&metadata) {
                    continue;
                }
                let relative = entry
                    .path()
                    .strip_prefix(root)
                    .map_err(|_| invalid("search escaped the project"))?
                    .to_string_lossy()
                    .replace('\\', "/");
                if metadata.is_dir() {
                    if ignored_directory(&name) {
                        continue;
                    }
                    if depth >= MAX_SEARCH_DEPTH {
                        truncated = true;
                        continue;
                    }
                    // Reserved device names, ':' or a trailing dot would fail
                    // validation when the directory is listed; skip them now.
                    if check_relative(&relative).is_err() {
                        omitted += 1;
                        continue;
                    }
                    stack.push((entry.path(), depth + 1));
                    continue;
                }
                if !metadata.is_file() {
                    continue;
                }
                if files_searched >= MAX_SEARCH_FILES || bytes_read >= MAX_SEARCH_BYTES {
                    truncated = true;
                    break 'directories;
                }
                let Ok(anchor) = Anchor::new(root, &relative, false) else {
                    omitted += 1;
                    continue;
                };
                let Ok(file) = anchor.open_read() else {
                    omitted += 1;
                    continue;
                };
                let limit = (64 * 1024).min(MAX_SEARCH_BYTES - bytes_read);
                let mut bytes = Vec::new();
                if file
                    .take((limit + 1) as u64)
                    .read_to_end(&mut bytes)
                    .is_err()
                {
                    omitted += 1;
                    continue;
                }
                files_searched += 1;
                let file_truncated = bytes.len() > limit;
                if file_truncated {
                    truncated = true;
                    bytes.truncate(limit);
                }
                bytes_read += bytes.len();
                if relative.contains(&args.query) {
                    matches.push(json!({"path":relative,"line":null,"text":"filename match"}));
                    if matches.len() >= args.max_results {
                        truncated = true;
                        break 'directories;
                    }
                }
                let Some(text) = utf8_text(&bytes, file_truncated) else {
                    continue;
                };
                if text.contains('\0') {
                    continue;
                }
                for (index, line) in text.lines().enumerate() {
                    if line.contains(&args.query) {
                        let excerpt: String = line.chars().take(2_000).collect();
                        matches.push(json!({"path":relative,"line":index + 1,"text":excerpt}));
                        if matches.len() >= args.max_results {
                            truncated = true;
                            break 'directories;
                        }
                    }
                }
            }
        }
        let mut result = ToolResult::completed(
            json!({"matches":matches,"files_searched":files_searched,"bytes_read":bytes_read,"omitted":omitted,"truncated":truncated}),
        );
        result.truncated = truncated;
        Ok(result)
    };
    match run() {
        Ok(result) => result,
        Err(error) => ToolResult::failed(error),
    }
}

#[derive(Clone, Debug)]
pub(super) struct PreparedEdit {
    path: String,
    before: Option<String>,
    after: String,
    content: String,
    diff: String,
}

impl PreparedEdit {
    pub(super) fn preview(&self) -> Value {
        json!({"path":self.path,"before_sha256":self.before,"after_sha256":self.after,"diff":self.diff})
    }
}

fn full_diff(path: &str, before: Option<&str>, after: &str) -> String {
    let old_name = if before.is_some() {
        format!("a/{path}")
    } else {
        "/dev/null".to_owned()
    };
    let before = before.unwrap_or("");
    let old_lines = before.lines().count();
    let new_lines = after.lines().count();
    let mut diff = format!(
        "--- {old_name}\n+++ b/{path}\n@@ -{},{} +{},{} @@\n",
        usize::from(old_lines > 0),
        old_lines,
        usize::from(new_lines > 0),
        new_lines
    );
    for (prefix, text) in [('-', before), ('+', after)] {
        for line in text.split_inclusive('\n') {
            diff.push(prefix);
            diff.push_str(line);
            if !line.ends_with('\n') {
                diff.push_str("\n\\ No newline at end of file\n");
            }
        }
    }
    diff
}

pub(super) fn prepare_edits(
    root: &Path,
    edits: &[EditArgs],
) -> Result<Vec<PreparedEdit>, ToolError> {
    if edits.is_empty() || edits.len() > 16 {
        return Err(invalid("edits must contain 1..=16 files"));
    }
    let mut paths = HashSet::new();
    let mut total_bytes = 0usize;
    let mut prepared = Vec::new();
    for edit in edits {
        if edit.content.len() > MAX_FILE_BYTES || edit.content.contains('\0') {
            return Err(invalid(
                "edit content must be bounded UTF-8 text without NUL characters",
            ));
        }
        if let Some(hash) = &edit.before_sha256
            && (hash.len() != 64
                || !hash
                    .bytes()
                    .all(|c| c.is_ascii_digit() || matches!(c, b'a'..=b'f')))
        {
            return Err(invalid(
                "before_sha256 must be a lowercase SHA-256 hex digest or null",
            ));
        }
        let anchor = Anchor::new(root, &edit.path, edit.before_sha256.is_none())?;
        // Normalize case on Windows to reject differently-cased aliases in a batch.
        let key = anchor.path.to_string_lossy().into_owned();
        #[cfg(windows)]
        let key = key.to_lowercase();
        if !paths.insert(key) {
            return Err(invalid("an edit batch may contain each file only once"));
        }
        let before = anchor.read_optional()?;
        if before.as_ref().map(|b| sha256(b)) != edit.before_sha256 {
            return Err(invalid(format!(
                "{} changed or already exists; read its current content before proposing another edit",
                edit.path
            )));
        }
        let before_text = before
            .as_ref()
            .map(|b| std::str::from_utf8(b))
            .transpose()
            .map_err(|_| invalid("cannot edit a non-UTF-8 file"))?;
        total_bytes =
            total_bytes.saturating_add(edit.content.len() + before.as_ref().map_or(0, Vec::len));
        if total_bytes > MAX_PATCH_BYTES {
            return Err(invalid(format!(
                "combined before/after patch content exceeds {MAX_PATCH_BYTES} bytes"
            )));
        }
        prepared.push(PreparedEdit {
            path: edit.path.clone(),
            before: edit.before_sha256.clone(),
            after: sha256(edit.content.as_bytes()),
            content: edit.content.clone(),
            diff: full_diff(&edit.path, before_text, &edit.content),
        });
    }
    Ok(prepared)
}

fn check_current(anchor: &Anchor, edit: &PreparedEdit) -> Result<(), ToolError> {
    if anchor.read_optional()?.as_ref().map(|b| sha256(b)) != edit.before {
        return Err(invalid(format!(
            "{} changed since approval; no replacement was made for this file",
            edit.path
        )));
    }
    Ok(())
}

pub(super) fn apply(root: &Path, edits: &[PreparedEdit], cancel: &CancellationToken) -> ToolResult {
    let mut changes = Vec::new();
    let run = || -> Result<ToolResult, ToolError> {
        let anchors = edits
            .iter()
            .map(|e| Anchor::new(root, &e.path, e.before.is_none()))
            .collect::<Result<Vec<_>, _>>()?;
        // Validate the complete batch before changing any file.
        for (anchor, edit) in anchors.iter().zip(edits) {
            check_current(anchor, edit)?;
        }
        if cancel.is_cancelled() {
            return Ok(ToolResult::cancelled());
        }
        for (anchor, edit) in anchors.iter().zip(edits) {
            if cancel.is_cancelled() {
                return Ok(ToolResult::cancelled());
            }
            replace(anchor, edit)?;
            changes.push(ChangedFile {
                path: edit.path.clone(),
                before_sha256: edit.before.clone(),
                after_sha256: edit.after.clone(),
            });
        }
        Ok(ToolResult::completed(
            json!({"files_changed":changes.len()}),
        ))
    };
    let mut run = run;
    let mut result = match run() {
        Ok(result) => result,
        Err(error) => ToolResult::failed(error),
    };
    result.changes = changes;
    result
}

#[cfg(windows)]
fn replace(anchor: &Anchor, edit: &PreparedEdit) -> Result<(), ToolError> {
    let parent = anchor
        .path
        .parent()
        .ok_or_else(|| invalid("missing parent directory"))?;
    let permissions = if edit.before.is_some() {
        Some(anchor.open_read()?.metadata()?.permissions())
    } else {
        None
    };
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(edit.content.as_bytes())?;
    if let Some(permissions) = permissions {
        temporary.as_file().set_permissions(permissions)?;
    }
    temporary.as_file().sync_all()?;
    check_current(anchor, edit)?;
    if edit.before.is_none() {
        temporary
            .persist_noclobber(&anchor.path)
            .map_err(|error| error.error)?;
    } else {
        temporary
            .persist(&anchor.path)
            .map_err(|error| error.error)?;
    }
    Ok(())
}

#[cfg(unix)]
fn replace(anchor: &Anchor, edit: &PreparedEdit) -> Result<(), ToolError> {
    use std::os::{
        fd::AsRawFd,
        unix::{ffi::OsStrExt, fs::PermissionsExt},
    };
    // Name generation uses tempfile's secure random naming, but creation and
    // rename stay descriptor-relative so a renamed parent cannot redirect writes.
    let temporary = tempfile::Builder::new()
        .prefix(".switchyard-edit-")
        .tempfile()?;
    let temp_name = temporary
        .path()
        .file_name()
        .expect("tempfile has a name")
        .to_os_string();
    drop(temporary);
    // A new file gets 0o666 minus the process umask, as editors and shells
    // create it. A replacement copies the original mode before the rename.
    let mode = if edit.before.is_none() { 0o666 } else { 0o600 };
    let mut file = unix_open_at(
        &anchor.parent,
        &temp_name,
        libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        mode,
    )?;
    let name_c =
        std::ffi::CString::new(anchor.name.as_bytes()).map_err(|_| invalid("NUL in path"))?;
    let temp_c = std::ffi::CString::new(temp_name.as_bytes())
        .map_err(|_| invalid("NUL in temporary path"))?;
    let outcome = (|| -> Result<(), ToolError> {
        file.write_all(edit.content.as_bytes())?;
        if edit.before.is_some() {
            let permissions = anchor.open_read()?.metadata()?.permissions();
            // Do not preserve setuid/setgid bits when replacing file contents.
            file.set_permissions(fs::Permissions::from_mode(permissions.mode() & 0o777))?;
        }
        file.sync_all()?;
        check_current(anchor, edit)?;
        if edit.before.is_none() {
            // linkat is an atomic create-if-absent operation on both Linux/macOS.
            // SAFETY: live directory descriptors and valid C strings.
            let result = unsafe {
                libc::linkat(
                    anchor.parent.as_raw_fd(),
                    temp_c.as_ptr(),
                    anchor.parent.as_raw_fd(),
                    name_c.as_ptr(),
                    0,
                )
            };
            if result != 0 {
                return Err(std::io::Error::last_os_error().into());
            }
        } else {
            // SAFETY: live directory descriptors and valid C strings.
            let result = unsafe {
                libc::renameat(
                    anchor.parent.as_raw_fd(),
                    temp_c.as_ptr(),
                    anchor.parent.as_raw_fd(),
                    name_c.as_ptr(),
                )
            };
            if result != 0 {
                return Err(std::io::Error::last_os_error().into());
            }
        }
        // The content has already been replaced at this point. Directory sync is
        // a durability improvement; do not report the file as unchanged if a
        // filesystem does not support syncing a directory handle.
        let _ = anchor.parent.sync_all();
        Ok(())
    })();
    // SAFETY: cleanup is relative to the same held parent descriptor. ENOENT is
    // expected after renameat. Never follow a replacement parent path.
    unsafe {
        libc::unlinkat(anchor.parent.as_raw_fd(), temp_c.as_ptr(), 0);
    }
    outcome
}
