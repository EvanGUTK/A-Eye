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
}
