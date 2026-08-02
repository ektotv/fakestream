//! Deciding whether a generated fixture on disk is still the one we want.
//!
//! Generation is slow enough that caching matters, and a cache with no
//! invalidation is worse than none: editing a fixture's definition would leave
//! the old file in place and serve it silently.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

/// Prefix for a file being written. Kept distinct so an interrupted run can be
/// swept up later rather than leaving rubbish behind for ever.
const PARTIAL_PREFIX: &str = ".partial-";

/// A short fingerprint of whatever produced a fixture.
///
/// Derived from the definition's debug output rather than a manual hash
/// implementation, so a new field is covered the moment it is added. Deriving
/// `Hash` would not work here, since the specs hold floats.
pub fn signature(definition: &impl std::fmt::Debug) -> String {
    let mut hasher = DefaultHasher::new();
    format!("{definition:?}").hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Where the fingerprint for a fixture is kept.
fn signature_path(target: &Path) -> PathBuf {
    let name = target
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    target.with_file_name(format!(".{name}.signature"))
}

/// Is the file on disk both present and made from this definition?
pub fn is_current(target: &Path, signature: &str) -> bool {
    if !target.exists() {
        return false;
    }

    match std::fs::read_to_string(signature_path(target)) {
        Ok(recorded) => recorded.trim() == signature,
        // No fingerprint means it predates this check, so treat it as stale
        // rather than trusting it.
        Err(_) => false,
    }
}

/// Record what produced a file, once it is complete.
pub fn record(target: &Path, signature: &str) -> std::io::Result<()> {
    std::fs::write(signature_path(target), signature)
}

/// The temporary name a fixture is written under.
///
/// The extension has to survive, because libavformat picks its muxer from the
/// filename. A temporary called `.partial` gets no muxer and the write fails
/// before it starts.
pub fn partial_name(target: &Path) -> PathBuf {
    match target.file_name().and_then(|name| name.to_str()) {
        Some(name) => target.with_file_name(format!("{PARTIAL_PREFIX}{name}")),
        None => target.with_file_name(PARTIAL_PREFIX),
    }
}

/// Delete half written files left by an interrupted run.
///
/// They are hidden, so they never show up in a directory listing and would
/// otherwise accumulate unnoticed.
pub fn sweep_partials(root: &Path) -> usize {
    let mut swept = 0;

    let Ok(entries) = std::fs::read_dir(root) else {
        return 0;
    };

    for entry in entries.flatten() {
        let path = entry.path();

        if path.is_dir() {
            swept += sweep_partials(&path);
            continue;
        }

        let is_partial = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(PARTIAL_PREFIX));

        if is_partial && std::fs::remove_file(&path).is_ok() {
            swept += 1;
        }
    }

    swept
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stands in for a fixture definition. The field is never read directly,
    /// only through Debug, which is exactly how signatures are derived.
    #[derive(Debug)]
    struct Definition {
        #[allow(dead_code)]
        seconds: f64,
    }

    #[test]
    fn the_same_definition_gives_the_same_signature() {
        let one = signature(&Definition { seconds: 30.0 });
        let two = signature(&Definition { seconds: 30.0 });
        assert_eq!(one, two);
    }

    #[test]
    fn a_changed_definition_gives_a_different_signature() {
        let before = signature(&Definition { seconds: 30.0 });
        let after = signature(&Definition { seconds: 31.0 });
        assert_ne!(before, after, "a changed fixture would have gone unnoticed");
    }

    #[test]
    fn the_temporary_name_keeps_the_extension() {
        // libavformat picks its muxer from the extension, so losing it breaks
        // generation before a byte is written.
        let partial = partial_name(Path::new("fixtures/vod/basic.mp4"));
        assert_eq!(partial.extension().and_then(|e| e.to_str()), Some("mp4"));
        assert_eq!(
            partial.parent(),
            Path::new("fixtures/vod")
                .parent()
                .map(|_| Path::new("fixtures/vod"))
        );
    }

    #[test]
    fn a_missing_file_is_never_current() {
        assert!(!is_current(Path::new("nowhere-at-all.mp4"), "abc"));
    }

    #[test]
    fn a_file_without_a_signature_is_stale() {
        let dir = std::env::temp_dir().join(format!("fakestream-cache-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let target = dir.join("thing.mp4");
        std::fs::write(&target, b"x").expect("write");

        assert!(
            !is_current(&target, "abc"),
            "an unfingerprinted file was trusted"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn recording_makes_a_file_current() {
        let dir = std::env::temp_dir().join(format!("fakestream-record-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let target = dir.join("thing.mp4");
        std::fs::write(&target, b"x").expect("write");

        record(&target, "abc").expect("record");
        assert!(is_current(&target, "abc"));
        assert!(!is_current(&target, "different"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn sweeping_removes_partials_and_leaves_the_rest() {
        let dir = std::env::temp_dir().join(format!("fakestream-sweep-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("vod")).expect("temp dir");
        std::fs::write(dir.join("vod/keep.mp4"), b"x").expect("write");
        std::fs::write(dir.join("vod/.partial-gone.mp4"), b"x").expect("write");

        assert_eq!(sweep_partials(&dir), 1);
        assert!(dir.join("vod/keep.mp4").exists());
        assert!(!dir.join("vod/.partial-gone.mp4").exists());

        std::fs::remove_dir_all(&dir).ok();
    }
}
