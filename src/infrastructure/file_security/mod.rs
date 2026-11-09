use std::path::Path;

/// Platform-agnostic file security operations.
///
/// Implementations MUST succeed or return an error — no silent no-ops.
pub trait FileSecurity {
    /// Restrict a file to owner-only read/write (0600 on Unix).
    fn restrict_key_permissions(&self, path: &Path) -> std::io::Result<()>;
}

#[cfg(unix)]
mod unix;

/// Get the file security provider for the current platform.
#[cfg(unix)]
pub fn get_file_security() -> impl FileSecurity {
    unix::UnixFileSecurity
}

/// Get the file security provider for the current platform.
#[cfg(not(unix))]
pub fn get_file_security() -> impl FileSecurity {
    UnsupportedFileSecurity
}

/// Convenience function: restrict a file to owner-only read/write.
///
/// Delegates to the platform file security provider.
pub fn restrict_key_permissions(path: &Path) -> std::io::Result<()> {
    get_file_security().restrict_key_permissions(path)
}

/// Writes a private key that no other user can read at any moment. Creating the
/// file first and restricting it afterwards leaves a window in which any process
/// could open it.
#[cfg(unix)]
pub fn write_private_file(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    // The mode above only applies to a new file.
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    file.write_all(contents)?;
    // On disk before any rename makes it the file others read.
    file.sync_all()
}

/// Fallback for unsupported platforms — always returns an error.
#[cfg(not(unix))]
struct UnsupportedFileSecurity;

#[cfg(not(unix))]
impl FileSecurity for UnsupportedFileSecurity {
    fn restrict_key_permissions(&self, _path: &Path) -> std::io::Result<()> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            format!(
                "File permission restriction not implemented for {}. \
                 Key files may be world-readable.",
                std::env::consts::OS
            ),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn a_private_file_is_owner_only_when_new() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ca.key");

        write_private_file(&path, b"key").unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"key");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o7777, 0o600);
    }

    #[test]
    fn a_private_file_closes_a_wider_existing_file_before_writing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ca.key");
        std::fs::write(&path, "old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        write_private_file(&path, b"new").unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o7777, 0o600);
    }
}
