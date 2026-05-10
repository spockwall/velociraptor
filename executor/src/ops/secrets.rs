//! Secrets handling: file-mode validation + zeroize-on-drop wrapper.
//!
//! `secrecy::SecretString` is the public surface — wrap any private key /
//! HMAC secret in it before passing through the system. `Display` and `Debug`
//! redact the inner value automatically.

use std::path::Path;

pub use secrecy::{ExposeSecret, SecretString};

/// Reject the credentials file if it's group- or world-readable. Mac/Linux
/// only — Windows skips the check (returns Ok).
pub fn ensure_owner_only<P: AsRef<Path>>(path: P) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = std::fs::metadata(&path)?;
        let mode = meta.mode() & 0o777;
        if mode & 0o077 != 0 {
            anyhow::bail!(
                "{}: file mode {:o} is group/world-readable; chmod 600 first",
                path.as_ref().display(),
                mode
            );
        }
    }
    let _ = path;
    Ok(())
}

/// Wrap a string in `SecretString`. Convenience around the constructor that
/// avoids exposing `secrecy` types at every call site.
pub fn wrap(s: impl Into<String>) -> SecretString {
    SecretString::from(s.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::NamedTempFile;

    #[test]
    fn rejects_group_readable() {
        let f = NamedTempFile::new().unwrap();
        std::fs::set_permissions(f.path(), std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(ensure_owner_only(f.path()).is_err());
    }

    #[test]
    fn accepts_owner_only() {
        let f = NamedTempFile::new().unwrap();
        std::fs::set_permissions(f.path(), std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(ensure_owner_only(f.path()).is_ok());
    }

    #[test]
    fn debug_redacts() {
        let s = wrap("topsecret");
        let dbg = format!("{:?}", s);
        assert!(!dbg.contains("topsecret"));
    }
}
