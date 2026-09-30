//! Cover versions: a 12-hex hash of the cover bytes on disk, stored in
//! `eras.cover_version` whenever a cover file is written (clients request
//! `/eras/:id/cover?v=<version>`, so the URL changes exactly when the image
//! does).

use std::path::Path;

use sha2::{Digest, Sha256};

/// Length of a cover version in hex digits.
pub const COVER_VERSION_LEN: usize = 12;

/// Version of a cover: the first 12 hex digits of the SHA-256 of its bytes.
pub fn cover_version_of(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    hex::encode(&digest[..COVER_VERSION_LEN / 2])
}

/// [`cover_version_of`] for the file at `path`; `None` when the file is
/// missing or empty.
pub fn cover_version_of_file(path: &Path) -> std::io::Result<Option<String>> {
    match std::fs::read(path) {
        Ok(bytes) if !bytes.is_empty() => Ok(Some(cover_version_of(&bytes))),
        Ok(_) => Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_versions_hash_the_content() {
        // SHA-256("abc") = ba7816bf8f01cfea…
        assert_eq!(cover_version_of(b"abc"), "ba7816bf8f01");
        assert_ne!(cover_version_of(b"abc"), cover_version_of(b"abd"));

        let dir = std::env::temp_dir().join(format!("yt-cover-version-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("1.avif");
        assert_eq!(cover_version_of_file(&path).unwrap(), None);
        std::fs::write(&path, b"").unwrap();
        assert_eq!(cover_version_of_file(&path).unwrap(), None);
        std::fs::write(&path, b"abc").unwrap();
        assert_eq!(
            cover_version_of_file(&path).unwrap().as_deref(),
            Some("ba7816bf8f01")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
