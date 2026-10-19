//! Disk-plaintext check (issue #4): look for the canary bytes under a set of
//! roots without printing any file content. A symlinked root is resolved once;
//! symlinks encountered while walking are never followed.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Files larger than this are skipped (and counted) rather than read.
const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Default)]
pub struct ScanResult {
    pub found_in: Vec<PathBuf>,
    pub files_scanned: u64,
    pub unreadable: u64,
    pub skipped_large: u64,
    pub roots_missing: Vec<PathBuf>,
    pub timed_out: bool,
}

impl ScanResult {
    /// A "not found" verdict is only trustworthy if every root was resolved and
    /// read and nothing was skipped.
    pub fn complete(&self) -> bool {
        !self.timed_out
            && self.unreadable == 0
            && self.skipped_large == 0
            && self.roots_missing.is_empty()
    }
}

pub fn contains(hay: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    let first = needle[0];
    let mut i = 0;
    while let Some(off) = hay[i..].iter().position(|&b| b == first) {
        let s = i + off;
        if hay.len() - s < needle.len() {
            return false;
        }
        if &hay[s..s + needle.len()] == needle {
            return true;
        }
        i = s + 1;
    }
    false
}

pub fn scan_for_canary(roots: &[PathBuf], needle: &[u8], budget: Duration) -> ScanResult {
    let deadline = Instant::now() + budget;
    let mut res = ScanResult::default();
    let mut stack: Vec<PathBuf> = Vec::new();
    for r in roots {
        // Follow a symlink only at the ROOT (common for dotfile setups); anything
        // found inside the walk is never followed. A root that cannot be resolved
        // is recorded as missing, which makes the scan incomplete.
        match fs::canonicalize(r) {
            Ok(real) => stack.push(real),
            Err(_) => res.roots_missing.push(r.clone()),
        }
    }
    while let Some(p) = stack.pop() {
        if Instant::now() > deadline {
            res.timed_out = true;
            break;
        }
        let Ok(md) = fs::symlink_metadata(&p) else {
            res.unreadable += 1;
            continue;
        };
        let ft = md.file_type();
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            match fs::read_dir(&p) {
                Ok(rd) => push_children(rd.map(|e| e.map(|e| e.path())), &mut stack, &mut res),
                Err(_) => res.unreadable += 1,
            }
        } else if ft.is_file() {
            scan_file(&p, md.len(), needle, &mut res);
        }
    }
    res
}

/// Queue directory children; an error yielded mid-iteration means an entry may
/// have been missed, so it is counted as unreadable (which makes the scan incomplete).
fn push_children(
    entries: impl Iterator<Item = std::io::Result<PathBuf>>,
    stack: &mut Vec<PathBuf>,
    res: &mut ScanResult,
) {
    for entry in entries {
        match entry {
            Ok(path) => stack.push(path),
            Err(_) => res.unreadable += 1,
        }
    }
}

fn scan_file(path: &Path, len: u64, needle: &[u8], res: &mut ScanResult) {
    match fs::File::open(path) {
        Ok(f) => scan_reader(path, f, len, MAX_FILE_BYTES, needle, res),
        Err(_) => res.unreadable += 1,
    }
}

/// Read at most `limit` bytes from `reader` and search them. The size limit is
/// enforced WHILE reading (`take(limit + 1)`), not just from the earlier metadata
/// length, so a file that grew (or is being appended to) cannot exhaust memory.
/// A reader that yields more than `limit` bytes counts as `skipped_large`.
fn scan_reader(
    path: &Path,
    reader: impl Read,
    len_hint: u64,
    limit: u64,
    needle: &[u8],
    res: &mut ScanResult,
) {
    if len_hint > limit {
        res.skipped_large += 1;
        return;
    }
    let mut buf = Vec::with_capacity(len_hint as usize);
    match reader.take(limit.saturating_add(1)).read_to_end(&mut buf) {
        Ok(n) if n as u64 > limit => res.skipped_large += 1,
        Ok(_) => {
            res.files_scanned += 1;
            if contains(&buf, needle) {
                res.found_in.push(path.to_path_buf());
            }
        }
        Err(_) => res.unreadable += 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("aeye-spike01-scan-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn contains_basics() {
        assert!(contains(b"xxabcxx", b"abc"));
        assert!(contains(b"abc", b"abc"));
        assert!(!contains(b"ab", b"abc"));
        assert!(!contains(b"aabbcc", b"abc"));
        assert!(contains(b"aaab", b"aab"));
    }

    #[test]
    fn positive_control_finds_planted_canary_and_negative_does_not() {
        let d = tmp("ctl");
        fs::create_dir_all(d.join("sub")).unwrap();
        fs::write(d.join("sub/plain.txt"), b"hello AEYE-SPIKE-CANARY-xyz bye").unwrap();
        fs::write(d.join("other.bin"), [0u8, 1, 2, 3]).unwrap();
        let hit = scan_for_canary(
            std::slice::from_ref(&d),
            b"AEYE-SPIKE-CANARY-xyz",
            Duration::from_secs(5),
        );
        assert_eq!(hit.found_in.len(), 1);
        assert!(hit.complete());
        let miss = scan_for_canary(
            std::slice::from_ref(&d),
            b"AEYE-SPIKE-CANARY-nope",
            Duration::from_secs(5),
        );
        assert!(miss.found_in.is_empty());
        assert_eq!(miss.files_scanned, 2);
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn missing_root_is_reported_not_silently_green() {
        let r = scan_for_canary(
            &[PathBuf::from("/nonexistent/aeye-spike-root")],
            b"x",
            Duration::from_secs(1),
        );
        assert_eq!(r.roots_missing.len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_not_followed() {
        let d = tmp("sym");
        let outside = tmp("sym-out");
        fs::write(outside.join("f"), b"AEYE-SPIKE-CANARY-sym").unwrap();
        std::os::unix::fs::symlink(&outside, d.join("link")).unwrap();
        let r = scan_for_canary(
            std::slice::from_ref(&d),
            b"AEYE-SPIKE-CANARY-sym",
            Duration::from_secs(5),
        );
        assert!(r.found_in.is_empty());
        fs::remove_dir_all(d).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_root_is_scanned_and_canary_found() {
        let real = tmp("root-real");
        let holder = tmp("root-holder");
        fs::write(real.join("f"), b"..AEYE-SPIKE-CANARY-root..").unwrap();
        let link = holder.join("config-link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let r = scan_for_canary(
            std::slice::from_ref(&link),
            b"AEYE-SPIKE-CANARY-root",
            Duration::from_secs(5),
        );
        assert_eq!(r.files_scanned, 1, "symlinked root was not read");
        assert_eq!(r.found_in.len(), 1);
        assert!(r.complete());
        fs::remove_dir_all(real).unwrap();
        fs::remove_dir_all(holder).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn missing_or_dangling_root_makes_scan_incomplete() {
        let d = tmp("incomplete");
        fs::write(d.join("f"), b"nothing here").unwrap();
        let dangling = d.join("dangling");
        std::os::unix::fs::symlink(d.join("does-not-exist"), &dangling).unwrap();
        for bad in [PathBuf::from("/nonexistent/aeye-spike-root"), dangling] {
            let r = scan_for_canary(
                &[d.clone(), bad.clone()],
                b"AEYE-SPIKE-CANARY-none",
                Duration::from_secs(5),
            );
            assert!(r.found_in.is_empty());
            assert_eq!(r.roots_missing, vec![bad.clone()], "{bad:?}");
            assert!(!r.complete(), "{bad:?} must make the scan incomplete");
        }
        // Control: a scan whose roots all resolve is complete.
        let ok = scan_for_canary(std::slice::from_ref(&d), b"x", Duration::from_secs(5));
        assert!(ok.complete());
        fs::remove_dir_all(d).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn symlink_inside_a_symlinked_root_walk_is_still_not_followed() {
        let outside = tmp("inner-out");
        fs::write(outside.join("f"), b"AEYE-SPIKE-CANARY-inner").unwrap();
        let real = tmp("inner-real");
        std::os::unix::fs::symlink(&outside, real.join("inner-link")).unwrap();
        let holder = tmp("inner-holder");
        let link = holder.join("root-link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let r = scan_for_canary(
            std::slice::from_ref(&link),
            b"AEYE-SPIKE-CANARY-inner",
            Duration::from_secs(5),
        );
        assert!(r.found_in.is_empty());
        assert!(r.complete());
        for d in [outside, real, holder] {
            fs::remove_dir_all(d).unwrap();
        }
    }

    /// Reader that claims nothing but yields far more than the limit, counting
    /// how much was actually pulled from it.
    struct Endless {
        pulled: std::rc::Rc<std::cell::Cell<u64>>,
    }

    impl Read for Endless {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            buf.fill(b'a');
            self.pulled.set(self.pulled.get() + buf.len() as u64);
            Ok(buf.len())
        }
    }

    #[test]
    fn file_that_outgrows_its_metadata_length_is_skipped_and_bounded() {
        let pulled = std::rc::Rc::new(std::cell::Cell::new(0));
        let mut res = ScanResult::default();
        // metadata said 10 bytes (under the limit of 1000) but the reader is endless.
        scan_reader(
            Path::new("grown"),
            Endless {
                pulled: pulled.clone(),
            },
            10,
            1000,
            b"zzz",
            &mut res,
        );
        assert_eq!(res.skipped_large, 1);
        assert_eq!(res.files_scanned, 0);
        assert!(!res.complete());
        // Never reads meaningfully past limit + 1 (one buffer-sized read at most).
        assert!(pulled.get() <= 1001 + 8192, "pulled {} bytes", pulled.get());
    }

    #[test]
    fn file_at_exactly_the_limit_is_scanned() {
        let mut res = ScanResult::default();
        let data = vec![b'a'; 1000];
        scan_reader(Path::new("exact"), &data[..], 1000, 1000, b"zzz", &mut res);
        assert_eq!((res.files_scanned, res.skipped_large), (1, 0));
        assert!(res.complete());
        let mut over = ScanResult::default();
        let data = vec![b'a'; 1001];
        scan_reader(Path::new("over"), &data[..], 1000, 1000, b"zzz", &mut over);
        assert_eq!((over.files_scanned, over.skipped_large), (0, 1));
    }

    #[test]
    fn directory_iteration_error_is_counted_and_makes_scan_incomplete() {
        let entries: Vec<std::io::Result<PathBuf>> = vec![
            Ok(PathBuf::from("/a")),
            Err(std::io::Error::other("EIO mid-iteration")),
            Ok(PathBuf::from("/b")),
        ];
        let mut stack = Vec::new();
        let mut res = ScanResult::default();
        push_children(entries.into_iter(), &mut stack, &mut res);
        assert_eq!(stack.len(), 2, "good entries are still queued");
        assert_eq!(res.unreadable, 1);
        assert!(!res.complete());
    }
}
