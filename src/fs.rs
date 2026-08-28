//! Unix file primitives for durable replacement, backups, and coordination.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const PRIVATE_MODE: u32 = 0o600;
static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);

/// Atomically replaces `path` with `bytes` using default file permissions.
///
/// The parent directory is created when necessary. Bytes are written to a
/// unique sibling temporary file and `sync_all` is called before rename. After
/// rename, the containing directory is synced so the new directory entry is
/// durable. If that final directory sync fails, the replacement has already
/// happened and remains in place; the error reports that durability is unknown.
pub fn atomic_replace(path: &Path, bytes: &[u8]) -> io::Result<()> {
    atomic_replace_inner(path, bytes, None, sync_directory)
}

/// Atomically replaces `path` with `bytes` using an exact Unix permission mode.
///
/// The temporary file is created with `mode` before any bytes are written, then
/// explicitly set to that mode to counter a restrictive process umask. File and
/// containing-directory durability semantics are identical to [`atomic_replace`].
pub fn atomic_replace_with_mode(path: &Path, bytes: &[u8], mode: u32) -> io::Result<()> {
    atomic_replace_inner(path, bytes, Some(mode), sync_directory)
}

/// Creates a durable private backup without overwriting an existing destination.
///
/// The source is copied into a fully synced `0o600` sibling temporary file. A
/// same-directory hard link publishes that complete file at `backup_path`; the
/// link operation fails atomically when the destination already exists. The
/// temporary link is then removed and the containing directory is synced. A
/// directory-sync failure is returned after publication without deleting the
/// backup.
pub fn nonclobber_backup(src: &Path, backup_path: &Path) -> io::Result<()> {
    let mut source = File::open(src)?;
    let parent = containing_directory(backup_path)?;
    fs::create_dir_all(parent)?;
    let temp = stage_file(backup_path, Some(PRIVATE_MODE), |staged| {
        io::copy(&mut source, staged).map(|_| ())
    })?;

    if let Err(error) = fs::hard_link(&temp, backup_path) {
        let _ = fs::remove_file(&temp);
        return Err(error);
    }
    fs::remove_file(&temp)?;
    sync_directory(parent)
}

/// An exclusive advisory lock held on an opened directory descriptor.
///
/// The directory itself is the lock object, so no deletable lock-file artifact
/// can split concurrent users into separate lock domains. Dropping the guard
/// closes its descriptor and releases the lock on every return or unwind path.
#[derive(Debug)]
pub struct DirectoryLock {
    _directory: File,
}

impl DirectoryLock {
    /// Creates `dir` when necessary, opens it, and blocks until its exclusive
    /// advisory `flock` is acquired. Interrupted acquisitions are retried.
    pub fn acquire(dir: &Path) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        let directory = File::open(dir)?;
        lock_exclusive(&directory)?;
        Ok(Self {
            _directory: directory,
        })
    }
}

fn atomic_replace_inner<F>(
    path: &Path,
    bytes: &[u8],
    mode: Option<u32>,
    sync_parent: F,
) -> io::Result<()>
where
    F: FnOnce(&Path) -> io::Result<()>,
{
    let parent = containing_directory(path)?;
    fs::create_dir_all(parent)?;
    let temp = stage_file(path, mode, |file| file.write_all(bytes))?;

    if let Err(error) = fs::rename(&temp, path) {
        let _ = fs::remove_file(&temp);
        return Err(error);
    }
    sync_parent(parent)
}

fn stage_file<F>(path: &Path, mode: Option<u32>, write: F) -> io::Result<PathBuf>
where
    F: FnOnce(&mut File) -> io::Result<()>,
{
    let parent = containing_directory(path)?;
    let (temp, mut file) = create_sibling_temp(path, parent, mode)?;
    let result = (|| {
        if let Some(mode) = mode {
            file.set_permissions(fs::Permissions::from_mode(mode))?;
        }
        write(&mut file)?;
        file.sync_all()
    })();
    drop(file);

    if let Err(error) = result {
        let _ = fs::remove_file(&temp);
        return Err(error);
    }
    Ok(temp)
}

fn create_sibling_temp(
    path: &Path,
    parent: &Path,
    mode: Option<u32>,
) -> io::Result<(PathBuf, File)> {
    let file_name = path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} has no file name", path.display()),
        )
    })?;

    loop {
        let sequence = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
        let mut name = OsString::from(".");
        name.push(file_name);
        name.push(format!(".crook.{}.{sequence}.tmp", std::process::id()));
        let temp = parent.join(name);

        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        if let Some(mode) = mode {
            options.mode(mode);
        }
        match options.open(&temp) {
            Ok(file) => return Ok((temp, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
}

fn containing_directory(path: &Path) -> io::Result<&Path> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} has no parent directory", path.display()),
        )
    })?;
    if parent.as_os_str().is_empty() {
        Ok(Path::new("."))
    } else {
        Ok(parent)
    }
}

fn sync_directory(directory: &Path) -> io::Result<()> {
    File::open(directory)?.sync_all()
}

fn lock_exclusive(file: &File) -> io::Result<()> {
    retry_interrupted(|| flock(file, libc::LOCK_EX))
}

fn flock(file: &File, operation: libc::c_int) -> io::Result<()> {
    // SAFETY: `file` owns this valid descriptor for the duration of the call.
    if unsafe { libc::flock(file.as_raw_fd(), operation) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn retry_interrupted<F>(mut operation: F) -> io::Result<()>
where
    F: FnMut() -> io::Result<()>,
{
    loop {
        match operation() {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            result => return result,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST_DIR: AtomicU64 = AtomicU64::new(0);

    struct TestDir(PathBuf);

    impl TestDir {
        fn new(tag: &str) -> Self {
            let sequence = NEXT_TEST_DIR.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("crook-fs-{tag}-{}-{sequence}", std::process::id()));
            fs::create_dir(&path).expect("create test directory");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn atomic_replace_creates_parent_and_replaces_complete_bytes() {
        let root = TestDir::new("replace");
        let path = root.path().join("nested/state.json");

        atomic_replace(&path, b"first").expect("create state");
        assert_eq!(fs::read(&path).expect("read first state"), b"first");

        atomic_replace(&path, b"second").expect("replace state");
        assert_eq!(fs::read(&path).expect("read second state"), b"second");
    }

    #[test]
    fn atomic_replace_applies_exact_private_mode() {
        let root = TestDir::new("mode");
        let path = root.path().join("private.json");

        atomic_replace_with_mode(&path, b"secret", 0o600).expect("write private state");

        let mode = fs::metadata(&path)
            .expect("state metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn containing_directory_is_synced_after_rename() {
        let root = TestDir::new("sync-order");
        let path = root.path().join("state.json");
        fs::write(&path, b"old").expect("write old state");
        let mut synced = false;

        atomic_replace_inner(&path, b"new", None, |directory| {
            assert_eq!(directory, root.path());
            assert_eq!(fs::read(&path).expect("read replacement"), b"new");
            synced = true;
            Ok(())
        })
        .expect("replace state");

        assert!(synced);
    }

    #[test]
    fn directory_sync_failure_is_returned_after_publication() {
        let root = TestDir::new("sync-failure");
        let path = root.path().join("state.json");

        let error = atomic_replace_inner(&path, b"new", None, |_| {
            Err(io::Error::other("directory sync refused"))
        })
        .expect_err("directory sync should fail");

        assert_eq!(error.to_string(), "directory sync refused");
        assert_eq!(fs::read(&path).expect("read published state"), b"new");
    }

    #[test]
    fn rename_failure_removes_sibling_temp() {
        let root = TestDir::new("rename-cleanup");
        let path = root.path().join("target");
        fs::create_dir(&path).expect("create conflicting directory");

        atomic_replace(&path, b"cannot replace directory").expect_err("rename should fail");

        let names = fs::read_dir(root.path())
            .expect("read test directory")
            .map(|entry| entry.expect("directory entry").file_name())
            .collect::<Vec<_>>();
        assert_eq!(names, vec![OsString::from("target")]);
    }

    #[test]
    fn backup_is_private_complete_and_nonclobbering() {
        let root = TestDir::new("backup");
        let source = root.path().join("config.toml");
        let backup = root.path().join("recovery/config.toml.backup");
        fs::write(&source, b"original").expect("write source");

        nonclobber_backup(&source, &backup).expect("create backup");

        assert_eq!(fs::read(&backup).expect("read backup"), b"original");
        let mode = fs::metadata(&backup)
            .expect("backup metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);

        fs::write(&source, b"changed").expect("change source");
        let error = nonclobber_backup(&source, &backup).expect_err("backup must not clobber");
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(
            fs::read(&backup).expect("read original backup"),
            b"original"
        );
        assert_eq!(
            fs::read_dir(backup.parent().expect("backup parent"))
                .expect("read backup directory")
                .count(),
            1
        );
    }

    #[test]
    fn directory_guard_locks_the_directory_inode_until_drop() {
        let root = TestDir::new("lock");
        let guard = DirectoryLock::acquire(root.path()).expect("acquire first lock");
        let contender = File::open(root.path()).expect("open lock contender");

        let error = flock(&contender, libc::LOCK_EX | libc::LOCK_NB)
            .expect_err("second descriptor must not acquire lock");
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);

        drop(guard);
        flock(&contender, libc::LOCK_EX | libc::LOCK_NB).expect("lock released by guard drop");
    }

    #[test]
    fn interrupted_lock_operations_are_retried() {
        let mut attempts = 0;

        retry_interrupted(|| {
            attempts += 1;
            if attempts < 3 {
                Err(io::Error::from(io::ErrorKind::Interrupted))
            } else {
                Ok(())
            }
        })
        .expect("operation eventually succeeds");

        assert_eq!(attempts, 3);
    }
}
