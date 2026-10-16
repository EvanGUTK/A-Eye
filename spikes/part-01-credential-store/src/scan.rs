//! Disk-plaintext check (issue #4): look for the canary bytes under a set of
//! roots without printing any file content. Symlinks are not followed.

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
    /// A "not found" verdict is only trustworthy if nothing was skipped.
    pub fn complete(&self) -> bool {
        !self.timed_out && self.unreadable == 0 && self.skipped_large == 0
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
        if r.exists() {
            stack.push(r.clone());
        } else {
            res.roots_missing.push(r.clone());
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
                Ok(rd) => stack.extend(rd.filter_map(Result::ok).map(|e| e.path())),
                Err(_) => res.unreadable += 1,
            }
        } else if ft.is_file() {
            scan_file(&p, md.len(), needle, &mut res);
        }
    }
    res
}

fn scan_file(path: &Path, len: u64, needle: &[u8], res: &mut ScanResult) {
    if len > MAX_FILE_BYTES {
        res.skipped_large += 1;
        return;
    }
    let mut buf = Vec::with_capacity(len as usize);
    match fs::File::open(path).and_then(|mut f| f.read_to_end(&mut buf)) {
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
}
