//! A-Eye spike part 01: credential store.
//!
//! Detects whether a Secret Service keyring is usable, exercises it (write, read,
//! delete, disk-plaintext scan), exercises the AES-256-GCM/Argon2id encrypted-file
//! fallback, runs the write/read/delete cycle on the ACTIVE path, and prints a
//! capability record. Exit code is 0 whenever the record was produced.
//!
//! Safety rules: only synthetic canary credentials are used; secret values are
//! never printed (only lengths and "match: true/false"); every Secret Service call
//! is bounded by a 15 s timeout; the encrypted file lives in a temp dir.

mod scan;

use spike_01_credential_store::{backend, filestore};

use std::ffi::OsString;
use std::fmt::Write as _;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use backend::{
    ActivePath, BackendError, CredentialBackend, SS_TIMEOUT, SecretServiceBackend, roundtrip,
    run_with_timeout, select_active,
};
use filestore::{FileStore, KdfParams};

/// Test-only passphrase. A real implementation prompts the user (never hardcode).
const TEST_PASSPHRASE: &str = "aeye-spike-01-TEST-ONLY-passphrase";
const LOCK: &str = include_str!("../Cargo.lock");

#[derive(Clone, Copy, PartialEq, Eq)]
enum Status {
    Pass,
    Fail,
    Unverified,
}

impl Status {
    fn label(self) -> &'static str {
        match self {
            Status::Pass => "PASS",
            Status::Fail => "FAIL",
            Status::Unverified => "UNVERIFIED",
        }
    }
}

struct Check {
    id: &'static str,
    name: &'static str,
    status: Status,
    reason: String,
}

#[derive(Default)]
struct Record {
    checks: Vec<Check>,
}

impl Record {
    fn add(
        &mut self,
        id: &'static str,
        name: &'static str,
        status: Status,
        reason: impl Into<String>,
    ) {
        let reason = reason.into();
        eprintln!("[{}] {id} {name}: {}", status.label(), reason);
        self.checks.push(Check {
            id,
            name,
            status,
            reason,
        });
    }
    fn pass(&mut self, id: &'static str, name: &'static str, reason: impl Into<String>) {
        self.add(id, name, Status::Pass, reason);
    }
    fn fail(&mut self, id: &'static str, name: &'static str, reason: impl Into<String>) {
        self.add(id, name, Status::Fail, reason);
    }
    fn unverified(&mut self, id: &'static str, name: &'static str, reason: impl Into<String>) {
        self.add(id, name, Status::Unverified, reason);
    }
    fn count(&self, s: Status) -> usize {
        self.checks.iter().filter(|c| c.status == s).count()
    }
}

fn locked_version(name: &str) -> String {
    let mut lines = LOCK.lines();
    let want = format!("name = \"{name}\"");
    while let Some(l) = lines.next() {
        if l == want
            && let Some(v) = lines.next().and_then(|v| v.strip_prefix("version = \""))
            && let Some(v) = v.strip_suffix('"')
        {
            return v.to_string();
        }
    }
    "unknown".to_string()
}

fn random_hex(n: usize) -> String {
    let mut buf = vec![0u8; n];
    getrandom::fill(&mut buf).expect("OS RNG unavailable (harness bug)");
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

fn command_output(program: &str, args: &[&str]) -> Option<String> {
    let (program, args): (String, Vec<String>) = (
        program.to_string(),
        args.iter().map(|s| s.to_string()).collect(),
    );
    run_with_timeout(Duration::from_secs(10), move || {
        Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    })
    .ok()
    .flatten()
}

/// Exit-status-only probe of the Secret Service through libsecret's `secret-tool`
/// (an independent client). The secret goes to /dev/null and is never read here.
fn secret_tool_has(account: &str) -> Option<bool> {
    let account = account.to_string();
    run_with_timeout(SS_TIMEOUT, move || {
        Command::new("secret-tool")
            .args([
                "lookup",
                "service",
                SecretServiceBackend::SERVICE,
                "username",
                &account,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .ok()
            .map(|s| s.success())
    })
    .ok()
    .flatten()
}

fn os_pretty_name() -> String {
    fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("PRETTY_NAME="))
                .map(|v| v.trim_matches('"').to_string())
        })
        .unwrap_or_else(|| "unknown".into())
}

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default.into())
}

/// Parse the `org.freedesktop.secrets` row of `busctl --user list`.
fn find_keyring_daemon() -> String {
    let Some(out) = command_output("busctl", &["--user", "--no-pager", "list"]) else {
        return "UNKNOWN (busctl missing, failed, or no session bus)".into();
    };
    match out
        .lines()
        .find(|l| l.starts_with("org.freedesktop.secrets"))
    {
        None => "NOT FOUND on session bus (no org.freedesktop.secrets name)".into(),
        Some(row) => {
            let cols: Vec<&str> = row.split_whitespace().collect();
            match (cols.get(1), cols.get(2)) {
                (Some(pid), Some(proc_name)) if pid.chars().all(|c| c.is_ascii_digit()) => {
                    format!("FOUND, running: {proc_name} (pid {pid})")
                }
                _ => "name present but activatable only (no running owner yet)".into(),
            }
        }
    }
}

fn hex_dump(bytes: &[u8]) -> String {
    let mut s = String::new();
    for (i, chunk) in bytes.chunks(16).enumerate() {
        let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02x}")).collect();
        let ascii: String = chunk
            .iter()
            .map(|&b| {
                if (0x20..0x7f).contains(&b) {
                    b as char
                } else {
                    '.'
                }
            })
            .collect();
        let _ = writeln!(s, "    {:04x}: {:<47}  |{ascii}|", i * 16, hex.join(" "));
    }
    s
}

fn status_of_err(e: &BackendError) -> Status {
    match e {
        BackendError::Timeout | BackendError::Unavailable(_) => Status::Unverified,
        _ => Status::Fail,
    }
}

fn err_reason(what: &str, e: &BackendError) -> String {
    match e {
        BackendError::Timeout => format!(
            "UNVERIFIED: keyring locked/unreachable ({what} exceeded {}s timeout)",
            SS_TIMEOUT.as_secs()
        ),
        BackendError::Unavailable(m) => {
            format!("UNVERIFIED: Secret Service unavailable ({what}: {m})")
        }
        other => format!("{what} failed: {other}"),
    }
}

struct SsOutcome {
    /// write + read + match all succeeded, i.e. the keyring is genuinely usable.
    usable: bool,
    /// Accounts that may still exist in the keyring and must be cleaned/verified.
    touched_accounts: Vec<String>,
}

/// Issue #4 + part of #6: exercise the Secret Service path.
fn exercise_secret_service(rec: &mut Record) -> SsOutcome {
    let ss = SecretServiceBackend::new(SS_TIMEOUT);
    let account = format!("test-provider:spike-{}", random_hex(6));
    let canary = format!("AEYE-SPIKE-CANARY-{}", random_hex(12));
    let mut out = SsOutcome {
        usable: false,
        touched_accounts: vec![account.clone()],
    };

    if let Err(e) = ss.store_init() {
        rec.add(
            "SS-01",
            "Secret Service store initialised (session bus + session open)",
            status_of_err(&e),
            err_reason("store init", &e),
        );
        for (id, name) in [
            ("SS-02", "Secret Service write"),
            ("SS-03", "Secret Service read + value match"),
            ("SS-04", "Independent cross-check via libsecret secret-tool"),
            (
                "SS-05",
                "Canary not plaintext on disk (keyrings + ~/.config)",
            ),
            ("SS-06", "Secret Service delete"),
            ("SS-07", "Entry gone after delete"),
        ] {
            rec.unverified(id, name, "not exercised: store init failed");
        }
        return out;
    }
    rec.pass(
        "SS-01",
        "Secret Service store initialised (session bus + session open)",
        "keyring crate zbus store connected",
    );

    let fp_before = keyring_dir_fingerprint();
    // SS-02 write
    match ss.set(&account, &canary) {
        Ok(()) => rec.pass(
            "SS-02",
            "Secret Service write",
            format!(
                "set_password OK (service=a-eye, account={account}, {} bytes)",
                canary.len()
            ),
        ),
        Err(e) => {
            rec.add(
                "SS-02",
                "Secret Service write",
                status_of_err(&e),
                err_reason("write", &e),
            );
            if e == BackendError::Timeout {
                eprintln!(
                    "note: if a late unlock completes the write, remove it with: secret-tool clear service a-eye username {account}"
                );
            }
            for (id, name) in [
                ("SS-03", "Secret Service read + value match"),
                ("SS-04", "Independent cross-check via libsecret secret-tool"),
                (
                    "SS-05",
                    "Canary not plaintext on disk (keyrings + ~/.config)",
                ),
                ("SS-06", "Secret Service delete"),
                ("SS-07", "Entry gone after delete"),
            ] {
                rec.unverified(id, name, "not exercised: write did not succeed");
            }
            return out;
        }
    }

    // SS-03 read + match
    let matched = match ss.get(&account) {
        Ok(got) => {
            let m = got.as_str() == canary;
            if m {
                rec.pass(
                    "SS-03",
                    "Secret Service read + value match",
                    format!("get_password OK, {} bytes, match: true", got.len()),
                );
            } else {
                rec.fail(
                    "SS-03",
                    "Secret Service read + value match",
                    format!("read {} bytes, match: false", got.len()),
                );
            }
            m
        }
        Err(e) => {
            rec.add(
                "SS-03",
                "Secret Service read + value match",
                status_of_err(&e),
                err_reason("read", &e),
            );
            false
        }
    };
    out.usable = matched;

    // SS-04 independent client
    match secret_tool_has(&account) {
        Some(true) => rec.pass("SS-04", "Independent cross-check via libsecret secret-tool", "item found by attributes service=a-eye username=<account> (exit status only; value not read)"),
        Some(false) => rec.fail("SS-04", "Independent cross-check via libsecret secret-tool", "secret-tool did not find the item the keyring crate wrote"),
        None => rec.unverified("SS-04", "Independent cross-check via libsecret secret-tool", "secret-tool missing or timed out"),
    }

    // SS-05 disk plaintext scan. Control first, so "not found" is meaningful.
    std::thread::sleep(Duration::from_millis(1500)); // let the daemon flush to disk
    rec.add_scan("SS-05", &canary, &fp_before);

    // SS-06 / SS-07 delete + gone
    match ss.delete(&account) {
        Ok(()) => rec.pass("SS-06", "Secret Service delete", "delete_credential OK"),
        Err(e) => rec.add(
            "SS-06",
            "Secret Service delete",
            status_of_err(&e),
            err_reason("delete", &e),
        ),
    }
    let gone_api = matches!(ss.get(&account), Err(BackendError::NotFound));
    let gone_tool = secret_tool_has(&account);
    match (gone_api, gone_tool) {
        (true, Some(false)) => rec.pass("SS-07", "Entry gone after delete", "keyring crate: NoEntry; secret-tool: not found"),
        (true, None) => rec.unverified("SS-07", "Entry gone after delete", "keyring crate: NoEntry; secret-tool cross-check unavailable"),
        _ => rec.fail("SS-07", "Entry gone after delete", format!("entry may remain: api_gone={gone_api}, secret-tool={gone_tool:?}; remove with: secret-tool clear service a-eye username {account}")),
    }
    out
}

/// (file name, size, mtime-nanos) for every file in ~/.local/share/keyrings; used to
/// prove the daemon actually persisted something before we claim "not plaintext".
fn keyring_dir_fingerprint() -> Vec<(String, u64, u128)> {
    let Some(home) = std::env::var_os("HOME") else {
        return Vec::new();
    };
    let mut v: Vec<(String, u64, u128)> =
        fs::read_dir(PathBuf::from(home).join(".local/share/keyrings"))
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter_map(|e| {
                let m = e.metadata().ok()?;
                let t = m
                    .modified()
                    .ok()?
                    .duration_since(std::time::UNIX_EPOCH)
                    .ok()?
                    .as_nanos();
                Some((e.file_name().to_string_lossy().into_owned(), m.len(), t))
            })
            .collect();
    v.sort();
    v
}

/// Classify a gnome-keyring file from its first bytes only (nothing is printed).
fn keyring_file_format(path: &Path) -> &'static str {
    use std::io::Read;
    let mut head = [0u8; 16];
    let n = fs::File::open(path)
        .and_then(|mut f| f.read(&mut head))
        .unwrap_or(0);
    let head = &head[..n];
    if head.starts_with(b"GnomeKeyring\n") {
        "gnome-keyring encrypted binary format"
    } else if head.starts_with(b"[keyring]") {
        "gnome-keyring UNENCRYPTED plaintext (INI) format"
    } else {
        "unrecognised format"
    }
}

impl Record {
    fn add_scan(&mut self, id: &'static str, canary: &str, fp_before: &[(String, u64, u128)]) {
        const NAME: &str = "Canary not plaintext on disk (keyrings + ~/.config)";
        // Positive control: the scanner must find a canary that IS on disk.
        let ctl_dir = std::env::temp_dir().join(format!("aeye-spike01-scanctl-{}", random_hex(4)));
        let control_ok = fs::create_dir_all(&ctl_dir)
            .and_then(|_| fs::write(ctl_dir.join("control.txt"), format!("x{canary}y")))
            .is_ok()
            && scan::scan_for_canary(
                std::slice::from_ref(&ctl_dir),
                canary.as_bytes(),
                Duration::from_secs(5),
            )
            .found_in
            .len()
                == 1;
        let _ = fs::remove_dir_all(&ctl_dir);
        if !control_ok {
            self.fail(
                id,
                NAME,
                "scanner positive control failed (harness bug); disk check not trustworthy",
            );
            return;
        }
        let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
            self.unverified(id, NAME, "HOME not set");
            return;
        };
        let roots = [home.join(".local/share/keyrings"), home.join(".config")];
        let r = scan::scan_for_canary(&roots, canary.as_bytes(), Duration::from_secs(60));
        let detail = format!(
            "scanned {} files; unreadable={}, skipped_large={}, missing_roots={}",
            r.files_scanned,
            r.unreadable,
            r.skipped_large,
            r.roots_missing.len()
        );
        if !r.found_in.is_empty() {
            let paths: Vec<String> = r.found_in.iter().map(|p| p.display().to_string()).collect();
            // Inspect only the first bytes of each hit to classify the on-disk format.
            let formats: Vec<&str> = r.found_in.iter().map(|p| keyring_file_format(p)).collect();
            self.fail(
                id,
                NAME,
                format!(
                    "PLAINTEXT CANARY FOUND in: {} [format: {}] ({detail}). The keyring backing store is not encrypted at rest on this machine; this is an environment property, not a defect in the keyring crate",
                    paths.join(", "),
                    formats.join(", ")
                ),
            );
        } else if keyring_dir_fingerprint() == fp_before {
            self.unverified(id, NAME, format!("canary not found, but no keyring file changed after the write, so persistence to disk was not observed (daemon may not have flushed yet); {detail}"));
        } else if !r.complete() {
            self.unverified(
                id,
                NAME,
                format!(
                    "canary not found, but scan INCOMPLETE (timed_out={}, unresolvable roots={:?}); {detail}",
                    r.timed_out, r.roots_missing
                ),
            );
        } else {
            self.pass(id, NAME, format!("canary not found; keyring files changed after the write (persisted); scanner positive control OK; {detail}"));
        }
    }
}

/// Issue #5: exercise the encrypted-file fallback in a temp dir.
fn exercise_file_fallback(rec: &mut Record, dir: &Path) {
    let path = dir.join("a-eye").join("credentials.enc");
    let store = FileStore::new(&path, TEST_PASSPHRASE, KdfParams::INTERACTIVE);
    let account = format!("test-provider:spike-{}", random_hex(6));
    let canary = format!("AEYE-SPIKE-CANARY-{}", random_hex(12));

    let t = Instant::now();
    if let Err(e) = store.set(&account, &canary) {
        rec.fail(
            "FS-01",
            "Encrypted-file write/read round trip",
            format!("write failed: {e}"),
        );
        return;
    }
    let kdf_ms = t.elapsed().as_millis();
    match store.get(&account) {
        Ok(got) if got.as_str() == canary => rec.pass(
            "FS-01",
            "Encrypted-file write/read round trip",
            format!(
                "{} bytes, match: true; write incl. Argon2id (m=19MiB,t=2,p=1) took {kdf_ms} ms",
                got.len()
            ),
        ),
        Ok(got) => rec.fail(
            "FS-01",
            "Encrypted-file write/read round trip",
            format!("read {} bytes, match: false", got.len()),
        ),
        Err(e) => rec.fail(
            "FS-01",
            "Encrypted-file write/read round trip",
            format!("read failed: {e}"),
        ),
    }

    let bytes = fs::read(&path).unwrap_or_default();
    let fmode = fs::metadata(&path)
        .map(|m| m.permissions().mode() & 0o777)
        .unwrap_or(0);
    let dmode = path
        .parent()
        .and_then(|p| fs::metadata(p).ok())
        .map(|m| m.permissions().mode() & 0o777)
        .unwrap_or(0);
    if fmode == 0o600 && dmode == 0o700 {
        rec.pass(
            "FS-02",
            "File mode 0600 (dir 0700)",
            format!("file {fmode:o}, dir {dmode:o}"),
        );
    } else {
        rec.fail(
            "FS-02",
            "File mode 0600 (dir 0700)",
            format!("file {fmode:o}, dir {dmode:o}"),
        );
    }

    let leaks = scan::contains(&bytes, canary.as_bytes())
        || scan::contains(&bytes, b"AEYE-SPIKE-CANARY")
        || scan::contains(&bytes, account.as_bytes());
    if bytes.is_empty() {
        rec.fail(
            "FS-03",
            "Canary and account name absent from file bytes",
            "file empty/unreadable (harness)",
        );
    } else if leaks {
        rec.fail(
            "FS-03",
            "Canary and account name absent from file bytes",
            "plaintext found in encrypted file",
        );
    } else {
        rec.pass(
            "FS-03",
            "Canary and account name absent from file bytes",
            format!("{} bytes scanned", bytes.len()),
        );
    }

    let header_ok = bytes.len() > filestore::HEADER_LEN
        && &bytes[0..4] == filestore::MAGIC
        && bytes[4] == filestore::FORMAT_VERSION;
    if header_ok {
        rec.pass(
            "FS-04",
            "Versioned header; ciphertext not human-readable",
            format!("magic=AEYC version={} (first 64 bytes below)", bytes[4]),
        );
        println!(
            "encrypted file head (xxd-style, first 64 bytes; header + start of ciphertext):\n{}",
            hex_dump(&bytes[..64.min(bytes.len())])
        );
    } else {
        rec.fail(
            "FS-04",
            "Versioned header; ciphertext not human-readable",
            "header missing/invalid",
        );
    }

    match FileStore::new(
        &path,
        "definitely-the-wrong-passphrase",
        KdfParams::INTERACTIVE,
    )
    .get(&account)
    {
        Err(BackendError::Other(m)) if m.contains("decryption failed") => rec.pass(
            "FS-05",
            "Wrong passphrase fails",
            "AES-GCM authentication failed, no plaintext returned",
        ),
        other => rec.fail(
            "FS-05",
            "Wrong passphrase fails",
            format!("unexpected result: ok={}", other.is_ok()),
        ),
    }

    let mut ct_flip = bytes.clone();
    let mid = filestore::HEADER_LEN + (ct_flip.len() - filestore::HEADER_LEN) / 2;
    ct_flip[mid] ^= 0x01;
    let mut hdr_flip = bytes.clone();
    hdr_flip[20] ^= 0x01; // salt byte
    let ct_rejected = filestore::decrypt_file(&ct_flip, TEST_PASSPHRASE.as_bytes()).is_err();
    let hdr_rejected = filestore::decrypt_file(&hdr_flip, TEST_PASSPHRASE.as_bytes()).is_err();
    if ct_rejected && hdr_rejected {
        rec.pass(
            "FS-06",
            "Tampered ciphertext and header rejected",
            "1-bit flip in ciphertext and in header salt both fail authentication",
        );
    } else {
        rec.fail(
            "FS-06",
            "Tampered ciphertext and header rejected",
            format!("ciphertext_rejected={ct_rejected}, header_rejected={hdr_rejected}"),
        );
    }

    // Fresh salt/nonce on every write.
    let before = bytes.clone();
    let rewrite = store.set(&account, &canary);
    let after = fs::read(&path).unwrap_or_default();
    if rewrite.is_ok()
        && after.len() >= 48
        && before.len() >= 48
        && before[20..36] != after[20..36]
        && before[36..48] != after[36..48]
    {
        rec.pass(
            "FS-07",
            "Fresh salt and nonce per write",
            "rewriting identical content changed both salt and nonce",
        );
    } else {
        rec.fail(
            "FS-07",
            "Fresh salt and nonce per write",
            "salt or nonce reused (or rewrite failed)",
        );
    }

    match store.delete(&account) {
        Ok(()) if matches!(store.get(&account), Err(BackendError::NotFound)) => rec.pass(
            "FS-08",
            "Encrypted-file delete",
            "entry removed, read-back NotFound",
        ),
        other => rec.fail(
            "FS-08",
            "Encrypted-file delete",
            format!("delete/verify failed: ok={}", other.is_ok()),
        ),
    }
}

const USAGE: &str = "usage: spike-01-credential-store [DIR]\n\n\
  Runs the credential-store spike and prints a capability record.\n\
  DIR (or env AEYE_SPIKE_CRED_DIR) is where the encrypted-file fallback writes\n\
  DIR/a-eye/credentials.enc. Default: a fresh temp directory. The spike REFUSES a DIR\n\
  that already contains an a-eye/ entry and only ever removes paths it created itself.\n\
  Env AEYE_SPIKE_FORCE_FILE=1 forces the encrypted-file path to be ACTIVE.\n\
  Options: -h, --help   show this text (any other option is rejected)";

#[derive(Debug, PartialEq, Eq)]
enum Cli {
    Help,
    Run(Option<PathBuf>),
}

/// Parse CLI args (without argv[0]) and the optional env directory.
/// Anything starting with '-' other than -h/--help is rejected, never treated as a path.
fn parse_cli(args: &[OsString], env_dir: Option<OsString>) -> Result<Cli, String> {
    let mut positional: Vec<&OsString> = Vec::new();
    for a in args {
        let text = a.to_string_lossy();
        if text == "-h" || text == "--help" {
            return Ok(Cli::Help);
        }
        if text.starts_with('-') {
            return Err(format!("unknown option {text:?}"));
        }
        positional.push(a);
    }
    if positional.len() > 1 {
        return Err("expected at most one directory argument".into());
    }
    let dir = env_dir
        .filter(|v| !v.is_empty())
        .or_else(|| positional.first().map(|p| (*p).clone()));
    match dir {
        Some(d) if d.is_empty() => Err("directory argument is empty".into()),
        Some(d) => Ok(Cli::Run(Some(PathBuf::from(d)))),
        None => Ok(Cli::Run(None)),
    }
}

/// The directory the encrypted-file phases may use, plus the directories this run
/// will create (deepest first) so cleanup removes only what the spike made.
struct Target {
    dir: PathBuf,
    created_dirs: Vec<PathBuf>,
}

impl Target {
    fn file(&self) -> PathBuf {
        self.dir.join("a-eye").join("credentials.enc")
    }
}

/// Fail fast, before anything is written, if the target already holds credentials
/// state we did not create.
fn prepare_target(dir: &Path) -> Result<Target, String> {
    let exists = |p: &Path| fs::symlink_metadata(p).is_ok();
    let sub = dir.join("a-eye");
    let file = sub.join("credentials.enc");
    if exists(&file) {
        return Err(format!(
            "refusing to run: {} already exists (the spike never overwrites or deletes existing credentials files)",
            file.display()
        ));
    }
    if exists(&sub) {
        return Err(format!(
            "refusing to run: {} already exists (choose an empty directory)",
            sub.display()
        ));
    }
    if exists(dir) && !dir.is_dir() {
        return Err(format!(
            "refusing to run: {} is not a directory",
            dir.display()
        ));
    }
    let mut created_dirs = Vec::new();
    let mut cur = Some(sub.as_path());
    while let Some(p) = cur {
        if exists(p) {
            break;
        }
        created_dirs.push(p.to_path_buf());
        cur = p.parent();
    }
    Ok(Target {
        dir: dir.to_path_buf(),
        created_dirs,
    })
}

/// Remove only what this run created: the credentials file (and its temp file),
/// then the directories recorded by `prepare_target`, deepest first. Non-empty
/// directories are left alone. Returns true if everything we created is gone.
fn cleanup_target(t: &Target) -> bool {
    let file = t.file();
    let _ = fs::remove_file(&file);
    let _ = fs::remove_file(file.with_extension("enc.tmp"));
    for d in &t.created_dirs {
        let _ = fs::remove_dir(d);
    }
    !file.exists() && t.created_dirs.iter().all(|d| !d.exists())
}

fn main() {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let cli = match parse_cli(&args, std::env::var_os("AEYE_SPIKE_CRED_DIR")) {
        Ok(Cli::Help) => {
            println!("{USAGE}");
            return;
        }
        Ok(Cli::Run(dir)) => dir,
        Err(msg) => {
            eprintln!("error: {msg}\n\n{USAGE}");
            std::process::exit(2);
        }
    };
    let dir = cli
        .unwrap_or_else(|| std::env::temp_dir().join(format!("aeye-spike-01-{}", random_hex(6))));
    let target = match prepare_target(&dir) {
        Ok(t) => t,
        Err(msg) => {
            eprintln!("error: {msg}");
            std::process::exit(2);
        }
    };
    run(&target);
}

fn run(target: &Target) {
    eprintln!("A-Eye spike part 01: credential store (progress on stderr, record on stdout)");
    let mut rec = Record::default();

    // ---- environment ----
    let os = os_pretty_name();
    let kernel = fs::read_to_string("/proc/sys/kernel/osrelease")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "unknown".into());
    let rustc = command_output("rustc", &["--version"]).unwrap_or_else(|| "unknown".into());
    let daemon = find_keyring_daemon();
    let pkgs = command_output("pacman", &["-Q", "libsecret", "gnome-keyring", "kwallet"])
        .unwrap_or_else(|| "n/a (pacman unavailable)".into());
    let session = format!(
        "XDG_SESSION_TYPE={} XDG_CURRENT_DESKTOP={} WAYLAND_DISPLAY={}",
        env_or("XDG_SESSION_TYPE", "unset"),
        env_or("XDG_CURRENT_DESKTOP", "unset"),
        env_or("WAYLAND_DISPLAY", "unset")
    );
    let bus = env_or("DBUS_SESSION_BUS_ADDRESS", "unset");

    let cred_dir = target.dir.clone();

    // ---- phases ----
    let ss = exercise_secret_service(&mut rec);
    exercise_file_fallback(&mut rec, &cred_dir);

    // ---- detection + ACTIVE path (issue #6) ----
    let force_file = std::env::var("AEYE_SPIKE_FORCE_FILE").is_ok_and(|v| v == "1");
    let active = select_active(ss.usable, force_file);
    let why = match (active, ss.usable, force_file) {
        (ActivePath::SecretService, ..) => "Secret Service round trip succeeded".to_string(),
        (ActivePath::EncryptedFile, _, true) => "forced by AEYE_SPIKE_FORCE_FILE=1".to_string(),
        (ActivePath::EncryptedFile, ..) => "Secret Service not usable (see SS-* rows)".to_string(),
    };
    let active_name = match active {
        ActivePath::SecretService => "SECRET SERVICE (OS keyring via keyring crate)",
        ActivePath::EncryptedFile => "ENCRYPTED FILE (AES-256-GCM + Argon2id fallback)",
    };
    let active_ss = SecretServiceBackend::new(SS_TIMEOUT);
    let active_file = FileStore::new(
        cred_dir.join("a-eye").join("credentials.enc"),
        TEST_PASSPHRASE,
        KdfParams::INTERACTIVE,
    );
    let active_backend: &dyn CredentialBackend = match active {
        ActivePath::SecretService => &active_ss,
        ActivePath::EncryptedFile => &active_file,
    };
    let act_account = format!("test-provider:spike-{}", random_hex(6));
    let act_canary = format!("AEYE-SPIKE-CANARY-{}", random_hex(12));
    let mut touched = ss.touched_accounts.clone();
    if active == ActivePath::SecretService {
        touched.push(act_account.clone());
    }
    match roundtrip(active_backend, &act_account, &act_canary) {
        Ok(()) => rec.pass(
            "ACT-01",
            "Write/read/delete cycle on ACTIVE backend",
            format!(
                "{}: set, get (match: true), delete, confirmed gone",
                active_backend.name()
            ),
        ),
        Err(f) => {
            let mut reason = err_reason("cycle", &f.err);
            let status = if f.entry_may_remain {
                reason.push_str(
                    "; an entry may remain, remove with: secret-tool clear service a-eye username <account>",
                );
                Status::Fail
            } else {
                status_of_err(&f.err)
            };
            rec.add(
                "ACT-01",
                "Write/read/delete cycle on ACTIVE backend",
                status,
                reason,
            );
        }
    }

    // ---- cleanup verification ("no token left behind") ----
    let mut left: Vec<String> = Vec::new();
    let mut unknown_left = false;
    for acct in &touched {
        match active_ss.get(acct) {
            Err(BackendError::NotFound) => {}
            Err(BackendError::Timeout) | Err(BackendError::Unavailable(_)) if !ss.usable => {
                unknown_left = true
            }
            _ => left.push(acct.clone()),
        }
    }
    let file_gone = cleanup_target(target);
    if !left.is_empty() {
        rec.fail("CLN-01", "No test entry left behind", format!("still present: {left:?} (remove with secret-tool clear service a-eye username <account>)"));
    } else if !file_gone {
        rec.fail(
            "CLN-01",
            "No test entry left behind",
            format!(
                "credentials file/dirs created by this run still exist under {}",
                cred_dir.display()
            ),
        );
    } else if unknown_left {
        rec.unverified("CLN-01", "No test entry left behind", "encrypted-file paths created by this run removed; Secret Service cleanup could not be verified (keyring unreachable/never written)");
    } else {
        rec.pass(
            "CLN-01",
            "No test entry left behind",
            "all Secret Service canary entries read back NotFound; encrypted-file paths created by this run removed",
        );
    }

    print_record(
        &rec,
        &os,
        &kernel,
        &rustc,
        &session,
        &bus,
        &daemon,
        &pkgs,
        active_name,
        &why,
        &cred_dir,
    );
    std::process::exit(0);
}

#[allow(clippy::too_many_arguments)]
fn print_record(
    rec: &Record,
    os: &str,
    kernel: &str,
    rustc: &str,
    session: &str,
    bus: &str,
    daemon: &str,
    pkgs: &str,
    active_name: &str,
    why: &str,
    cred_dir: &Path,
) {
    println!(
        "\n==================== A-Eye spike 01: credential store capability record ===================="
    );
    println!("OS:                {os}");
    println!("Kernel:            {kernel}");
    println!("Session/desktop:   {session}");
    println!("D-Bus session bus: {bus}");
    println!("Keyring daemon:    {daemon}");
    println!("Distro packages:   {}", pkgs.replace('\n', "; "));
    println!("rustc:             {rustc}");
    println!("Crate versions (from Cargo.lock; pinned exact in Cargo.toml):");
    for c in [
        "keyring",
        "keyring-core",
        "zbus-secret-service-keyring-store",
        "secret-service",
        "zbus",
        "aes-gcm",
        "argon2",
        "zeroize",
        "getrandom",
    ] {
        println!("  {c:<36} {}", locked_version(c));
    }
    println!(
        "Credential model:  service=\"a-eye\", account=\"<provider>:<account_id>\" (synthetic canaries only)"
    );
    println!(
        "Encrypted file:    {}/a-eye/credentials.enc (removed at end; only paths created by this run)",
        cred_dir.display()
    );
    println!("\nACTIVE STORAGE PATH: {active_name}\n  reason: {why}\n");

    println!("{:<7} {:<58} {:<11} reason", "id", "check", "result");
    println!("{}", "-".repeat(150));
    for c in &rec.checks {
        println!(
            "{:<7} {:<58} {:<11} {}",
            c.id,
            c.name,
            c.status.label(),
            c.reason
        );
    }
    println!("{}", "-".repeat(150));
    println!(
        "summary: {} PASS, {} FAIL, {} UNVERIFIED",
        rec.count(Status::Pass),
        rec.count(Status::Fail),
        rec.count(Status::Unverified)
    );

    println!("\nWHAT WAS PROVED (PASS rows only):");
    for c in rec.checks.iter().filter(|c| c.status == Status::Pass) {
        println!("  - {}: {}", c.id, c.name);
    }
    println!("\nWHAT FAILED OR IS UNKNOWN:");
    let mut any = false;
    for c in rec.checks.iter().filter(|c| c.status != Status::Pass) {
        any = true;
        println!(
            "  - {} {}: {} ({})",
            c.id,
            c.name,
            c.status.label(),
            c.reason
        );
    }
    if !any {
        println!("  - (no failing or unverified checks in this run)");
    }
    println!(
        "  - Not tested by this spike: kwallet (KDE) as Secret Service provider; a fresh-boot locked login keyring (GUI unlock flow);"
    );
    println!(
        "    concurrent access from several processes; headless/no-session-bus operation unless re-run with DBUS_SESSION_BUS_ADDRESS overridden;"
    );
    println!(
        "    interactive passphrase prompting and key caching (the spike takes a constant passphrase by design)."
    );
    println!(
        "============================================================================================="
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("aeye-spike01-main-{tag}-{}", random_hex(4)));
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn cli_rejects_unknown_flags_and_never_treats_them_as_paths() {
        for bad in ["--bogus", "-x", "--dir=/tmp/x", "-"] {
            let r = parse_cli(&os(&[bad]), None);
            assert!(r.is_err(), "{bad} accepted: {r:?}");
        }
        assert!(parse_cli(&os(&["/tmp/a", "/tmp/b"]), None).is_err());
        assert!(parse_cli(&os(&[""]), None).is_err());
    }

    #[test]
    fn cli_help_and_directory_forms() {
        assert_eq!(parse_cli(&os(&["--help"]), None), Ok(Cli::Help));
        assert_eq!(parse_cli(&os(&["-h"]), None), Ok(Cli::Help));
        assert_eq!(parse_cli(&[], None), Ok(Cli::Run(None)));
        assert_eq!(
            parse_cli(&os(&["/tmp/d"]), None),
            Ok(Cli::Run(Some(PathBuf::from("/tmp/d"))))
        );
        assert_eq!(
            parse_cli(&[], Some("/tmp/env".into())),
            Ok(Cli::Run(Some(PathBuf::from("/tmp/env"))))
        );
    }

    #[test]
    fn preexisting_credentials_file_is_refused_and_left_untouched() {
        let d = tmp("pre-file");
        let sub = d.join("a-eye");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("credentials.enc"), b"USER-DATA-DO-NOT-TOUCH").unwrap();
        let err = prepare_target(&d).err().expect("must refuse");
        assert!(err.contains("refusing"), "{err}");
        assert_eq!(
            fs::read(sub.join("credentials.enc")).unwrap(),
            b"USER-DATA-DO-NOT-TOUCH"
        );
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn preexisting_a_eye_dir_is_refused() {
        let d = tmp("pre-dir");
        fs::create_dir_all(d.join("a-eye")).unwrap();
        assert!(prepare_target(&d).is_err());
        assert!(d.join("a-eye").is_dir(), "refusal must not remove it");
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn cleanup_removes_only_what_the_run_created() {
        // Existing parent with user files: only a-eye/ (ours) may be removed.
        let d = tmp("cleanup");
        fs::write(d.join("keep.txt"), b"keep").unwrap();
        let t = prepare_target(&d).unwrap();
        assert_eq!(t.created_dirs, vec![d.join("a-eye")]);
        fs::create_dir_all(d.join("a-eye")).unwrap();
        fs::write(t.file(), b"ours").unwrap();
        assert!(cleanup_target(&t));
        assert!(!d.join("a-eye").exists());
        assert_eq!(fs::read(d.join("keep.txt")).unwrap(), b"keep");

        // Missing parent chain: all created dirs go, nothing else.
        let nested = d.join("x").join("y");
        let t = prepare_target(&nested).unwrap();
        assert_eq!(t.created_dirs.len(), 3);
        fs::create_dir_all(t.file().parent().unwrap()).unwrap();
        fs::write(t.file(), b"ours").unwrap();
        assert!(cleanup_target(&t));
        assert!(!d.join("x").exists());
        assert!(d.join("keep.txt").exists());
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn non_directory_target_is_refused() {
        let d = tmp("notdir");
        let f = d.join("file");
        fs::write(&f, b"x").unwrap();
        assert!(prepare_target(&f).is_err());
        fs::remove_dir_all(d).unwrap();
    }
}
