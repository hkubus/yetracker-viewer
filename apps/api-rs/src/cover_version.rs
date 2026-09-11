//! `sha1(imageUrl ?? "")[..12]` with an LRU of 1000 entries (`util/coverVersion.ts`).

use std::num::NonZeroUsize;
use std::sync::Mutex;

use lru::LruCache;
use sha1::{Digest, Sha1};

pub struct CoverVersions {
    cache: Mutex<LruCache<String, String>>,
}

impl CoverVersions {
    pub fn new() -> Self {
        Self {
            cache: Mutex::new(LruCache::new(NonZeroUsize::new(1000).expect("non-zero capacity"))),
        }
    }

    pub fn get(&self, image_url: Option<&str>) -> String {
        let key = image_url.unwrap_or("");
        let mut cache = self.cache.lock().expect("cover version cache poisoned");
        if let Some(version) = cache.get(key) {
            return version.clone();
        }
        let version = sha1_hex(key)[..12].to_string();
        cache.put(key.to_string(), version.clone());
        version
    }
}

impl Default for CoverVersions {
    fn default() -> Self {
        Self::new()
    }
}

pub fn sha1_hex(value: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(value.as_bytes());
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_first_twelve_hex_chars() {
        let versions = CoverVersions::new();
        let version = versions.get(Some("https://example.com/a.avif"));
        assert_eq!(version.len(), 12);
        assert_eq!(version, sha1_hex("https://example.com/a.avif")[..12].to_string());
        // Cached path returns the same value.
        assert_eq!(versions.get(Some("https://example.com/a.avif")), version);
        // Null maps to the empty-string digest.
        assert_eq!(versions.get(None), sha1_hex("")[..12].to_string());
    }
}
