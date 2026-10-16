//! Encrypted-file fallback (issue #5).
//!
//! File layout (all integers big-endian), 48-byte header followed by AEAD output:
//!
//! ```text
//! 0..4    magic  "AEYC"
//! 4       format version (1)
//! 5       KDF id (1 = Argon2id v0x13)
//! 6       AEAD id (1 = AES-256-GCM)
//! 7       reserved (0)
//! 8..12   Argon2 m_cost (KiB)
//! 12..16  Argon2 t_cost
//! 16..20  Argon2 p_cost
//! 20..36  salt   (16 random bytes, fresh on every write)
//! 36..48  nonce  (12 random bytes, fresh on every write)
//! 48..    AES-256-GCM ciphertext || 16-byte tag
//! ```
//!
//! The whole 48-byte header is bound into the GCM tag as associated data, so a
//! tampered header (including the KDF cost parameters) fails authentication.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use argon2::{Algorithm, Argon2, Params, Version};
use zeroize::{Zeroize, Zeroizing};

use crate::backend::{BackendError, CredentialBackend};

pub const MAGIC: &[u8; 4] = b"AEYC";
pub const FORMAT_VERSION: u8 = 1;
const KDF_ARGON2ID: u8 = 1;
const AEAD_AES256GCM: u8 = 1;
pub const SALT_LEN: usize = 16;
pub const NONCE_LEN: usize = 12;
pub const HEADER_LEN: usize = 4 + 4 + 12 + SALT_LEN + NONCE_LEN; // 48
const TAG_LEN: usize = 16;

/// Upper bounds applied to *untrusted* header values so a hostile file cannot
/// request absurd Argon2 work before authentication.
const MAX_M_COST_KIB: u32 = 256 * 1024;
const MAX_T_COST: u32 = 10;
const MAX_P_COST: u32 = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KdfParams {
    pub m_cost_kib: u32,
    pub t_cost: u32,
    pub p_cost: u32,
}

impl KdfParams {
    /// OWASP-style Argon2id minimum profile (19 MiB, t=2, p=1).
    pub const INTERACTIVE: KdfParams = KdfParams {
        m_cost_kib: 19 * 1024,
        t_cost: 2,
        p_cost: 1,
    };
    /// Deliberately weak; unit tests only.
    #[cfg(test)]
    pub const FAST_TEST: KdfParams = KdfParams {
        m_cost_kib: 256,
        t_cost: 1,
        p_cost: 1,
    };
}

#[derive(Debug, PartialEq, Eq)]
pub enum FileStoreError {
    Io(String),
    BadFormat(&'static str),
    /// Wrong passphrase *or* modified file: AES-GCM cannot (and must not) tell them apart.
    DecryptFailed,
    Internal(String),
}

impl fmt::Display for FileStoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FileStoreError::Io(e) => write!(f, "I/O error: {e}"),
            FileStoreError::BadFormat(e) => write!(f, "bad file format: {e}"),
            FileStoreError::DecryptFailed => {
                write!(f, "decryption failed (wrong passphrase or file modified)")
            }
            FileStoreError::Internal(e) => write!(f, "internal error: {e}"),
        }
    }
}

impl std::error::Error for FileStoreError {}

impl From<FileStoreError> for BackendError {
    fn from(e: FileStoreError) -> Self {
        BackendError::Other(e.to_string())
    }
}

type Secrets = BTreeMap<String, Zeroizing<String>>;

pub struct FileStore {
    path: PathBuf,
    passphrase: Zeroizing<String>,
    params: KdfParams,
}

impl FileStore {
    pub fn new(path: impl Into<PathBuf>, passphrase: &str, params: KdfParams) -> Self {
        FileStore {
            path: path.into(),
            passphrase: Zeroizing::new(passphrase.to_string()),
            params,
        }
    }

    #[cfg(test)]
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn load(&self) -> Result<Secrets, FileStoreError> {
        match fs::read(&self.path) {
            Ok(bytes) => decrypt_file(&bytes, self.passphrase.as_bytes()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Secrets::new()),
            Err(e) => Err(FileStoreError::Io(e.to_string())),
        }
    }

    fn save(&self, secrets: &Secrets) -> Result<(), FileStoreError> {
        let bytes = encrypt_file(secrets, self.passphrase.as_bytes(), self.params)?;
        write_private_atomic(&self.path, &bytes)
    }
}

impl CredentialBackend for FileStore {
    fn name(&self) -> &'static str {
        "encrypted-file (AES-256-GCM + Argon2id)"
    }

    fn set(&self, account: &str, secret: &str) -> Result<(), BackendError> {
        let mut all = self.load()?;
        all.insert(account.to_string(), Zeroizing::new(secret.to_string()));
        Ok(self.save(&all)?)
    }

    fn get(&self, account: &str) -> Result<Zeroizing<String>, BackendError> {
        let mut all = self.load()?;
        all.remove(account).ok_or(BackendError::NotFound)
    }

    fn delete(&self, account: &str) -> Result<(), BackendError> {
        let mut all = self.load()?;
        if all.remove(account).is_none() {
            return Err(BackendError::NotFound);
        }
        Ok(self.save(&all)?)
    }
}

fn random<const N: usize>() -> Result<[u8; N], FileStoreError> {
    let mut buf = [0u8; N];
    getrandom::fill(&mut buf).map_err(|e| FileStoreError::Internal(format!("rng: {e}")))?;
    Ok(buf)
}

fn derive_key(
    passphrase: &[u8],
    salt: &[u8],
    p: KdfParams,
) -> Result<Zeroizing<[u8; 32]>, FileStoreError> {
    let params = Params::new(p.m_cost_kib, p.t_cost, p.p_cost, Some(32))
        .map_err(|_| FileStoreError::BadFormat("invalid Argon2 parameters"))?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = Zeroizing::new([0u8; 32]);
    argon
        .hash_password_into(passphrase, salt, &mut key[..])
        .map_err(|e| FileStoreError::Internal(format!("argon2: {e}")))?;
    Ok(key)
}

fn serialize(secrets: &Secrets) -> Result<Zeroizing<Vec<u8>>, FileStoreError> {
    // Reserve the exact size up front: a growing Vec would reallocate and leave
    // un-zeroized plaintext copies in freed heap blocks.
    let total: usize = 4 + secrets
        .iter()
        .map(|(k, v)| 2 + k.len() + 4 + v.len())
        .sum::<usize>();
    let mut out = Zeroizing::new(Vec::with_capacity(total));
    let count = u32::try_from(secrets.len())
        .map_err(|_| FileStoreError::Internal("too many entries".into()))?;
    out.extend_from_slice(&count.to_be_bytes());
    for (k, v) in secrets {
        let kl = u16::try_from(k.len())
            .map_err(|_| FileStoreError::Internal("account too long".into()))?;
        let vl = u32::try_from(v.len())
            .map_err(|_| FileStoreError::Internal("secret too long".into()))?;
        out.extend_from_slice(&kl.to_be_bytes());
        out.extend_from_slice(k.as_bytes());
        out.extend_from_slice(&vl.to_be_bytes());
        out.extend_from_slice(v.as_bytes());
    }
    debug_assert_eq!(out.len(), total);
    Ok(out)
}

/// UTF-8 decode that wipes the rejected bytes (they may be secret material).
fn utf8_or_zeroize(bytes: Vec<u8>, what: &'static str) -> Result<String, FileStoreError> {
    String::from_utf8(bytes).map_err(|e| {
        let mut raw = e.into_bytes();
        raw.zeroize();
        FileStoreError::BadFormat(what)
    })
}

struct Cursor<'a>(&'a [u8]);

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], FileStoreError> {
        if self.0.len() < n {
            return Err(FileStoreError::BadFormat("truncated payload"));
        }
        let (head, tail) = self.0.split_at(n);
        self.0 = tail;
        Ok(head)
    }
}

fn deserialize(bytes: &[u8]) -> Result<Secrets, FileStoreError> {
    let mut c = Cursor(bytes);
    let count = u32::from_be_bytes(c.take(4)?.try_into().expect("4 bytes"));
    let mut out = Secrets::new();
    for _ in 0..count {
        let kl = u16::from_be_bytes(c.take(2)?.try_into().expect("2 bytes")) as usize;
        let k = utf8_or_zeroize(c.take(kl)?.to_vec(), "account not utf-8")?;
        let vl = u32::from_be_bytes(c.take(4)?.try_into().expect("4 bytes")) as usize;
        let v = utf8_or_zeroize(c.take(vl)?.to_vec(), "secret not utf-8")?;
        out.insert(k, Zeroizing::new(v));
    }
    if !c.0.is_empty() {
        return Err(FileStoreError::BadFormat("trailing bytes"));
    }
    Ok(out)
}

pub fn encrypt_file(
    secrets: &Secrets,
    passphrase: &[u8],
    params: KdfParams,
) -> Result<Vec<u8>, FileStoreError> {
    let salt = random::<SALT_LEN>()?;
    let nonce_bytes = random::<NONCE_LEN>()?;

    let mut header = Vec::with_capacity(HEADER_LEN);
    header.extend_from_slice(MAGIC);
    header.extend_from_slice(&[FORMAT_VERSION, KDF_ARGON2ID, AEAD_AES256GCM, 0]);
    header.extend_from_slice(&params.m_cost_kib.to_be_bytes());
    header.extend_from_slice(&params.t_cost.to_be_bytes());
    header.extend_from_slice(&params.p_cost.to_be_bytes());
    header.extend_from_slice(&salt);
    header.extend_from_slice(&nonce_bytes);
    debug_assert_eq!(header.len(), HEADER_LEN);

    let key = derive_key(passphrase, &salt, params)?;
    let cipher = Aes256Gcm::new_from_slice(&key[..])
        .map_err(|_| FileStoreError::Internal("bad key length".into()))?;
    let nonce = Nonce::try_from(&nonce_bytes[..])
        .map_err(|_| FileStoreError::Internal("bad nonce length".into()))?;
    let plaintext = serialize(secrets)?;
    let ct = cipher
        .encrypt(
            &nonce,
            Payload {
                msg: &plaintext,
                aad: &header,
            },
        )
        .map_err(|_| FileStoreError::Internal("encrypt failed".into()))?;

    let mut out = header;
    out.extend_from_slice(&ct);
    Ok(out)
}

pub fn decrypt_file(bytes: &[u8], passphrase: &[u8]) -> Result<Secrets, FileStoreError> {
    if bytes.len() < HEADER_LEN + TAG_LEN {
        return Err(FileStoreError::BadFormat("file too short"));
    }
    let (header, ct) = bytes.split_at(HEADER_LEN);
    if &header[0..4] != MAGIC {
        return Err(FileStoreError::BadFormat("bad magic"));
    }
    if header[4] != FORMAT_VERSION {
        return Err(FileStoreError::BadFormat("unsupported format version"));
    }
    if header[5] != KDF_ARGON2ID || header[6] != AEAD_AES256GCM || header[7] != 0 {
        return Err(FileStoreError::BadFormat("unsupported kdf/aead id"));
    }
    let u32_at = |i: usize| u32::from_be_bytes(header[i..i + 4].try_into().expect("4 bytes"));
    let params = KdfParams {
        m_cost_kib: u32_at(8),
        t_cost: u32_at(12),
        p_cost: u32_at(16),
    };
    if params.m_cost_kib > MAX_M_COST_KIB
        || params.t_cost == 0
        || params.t_cost > MAX_T_COST
        || params.p_cost == 0
        || params.p_cost > MAX_P_COST
    {
        return Err(FileStoreError::BadFormat("kdf parameters out of bounds"));
    }
    let salt = &header[20..20 + SALT_LEN];
    let nonce = Nonce::try_from(&header[20 + SALT_LEN..HEADER_LEN])
        .map_err(|_| FileStoreError::BadFormat("bad nonce"))?;

    let key = derive_key(passphrase, salt, params)?;
    let cipher = Aes256Gcm::new_from_slice(&key[..])
        .map_err(|_| FileStoreError::Internal("bad key length".into()))?;
    let plaintext = Zeroizing::new(
        cipher
            .decrypt(
                &nonce,
                Payload {
                    msg: ct,
                    aad: header,
                },
            )
            .map_err(|_| FileStoreError::DecryptFailed)?,
    );
    deserialize(&plaintext)
}

/// Write `bytes` to `path` with mode 0600 (parent dir 0700 if created), via a
/// temp file in the same directory and an atomic rename.
fn write_private_atomic(path: &Path, bytes: &[u8]) -> Result<(), FileStoreError> {
    let io = |e: std::io::Error| FileStoreError::Io(e.to_string());
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(parent)
        .map_err(io)?;
    let tmp = path.with_extension("enc.tmp");
    let _ = fs::remove_file(&tmp);
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(io)?;
    f.write_all(bytes).map_err(io)?;
    f.sync_all().map_err(io)?;
    drop(f);
    fs::rename(&tmp, path).map_err(io)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    const PASS: &str = "unit-test-passphrase";
    const CANARY: &str = "AEYE-SPIKE-CANARY-unit-0123456789abcdef";

    fn temp_dir(tag: &str) -> PathBuf {
        let mut r = [0u8; 8];
        getrandom::fill(&mut r).unwrap();
        let hex: String = r.iter().map(|b| format!("{b:02x}")).collect();
        let d = std::env::temp_dir().join(format!("aeye-spike01-test-{tag}-{hex}"));
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn store(dir: &Path, pass: &str) -> FileStore {
        FileStore::new(dir.join("credentials.enc"), pass, KdfParams::FAST_TEST)
    }

    #[test]
    fn round_trip_set_get_delete() {
        let dir = temp_dir("rt");
        let s = store(&dir, PASS);
        s.set("test-provider:a", CANARY).unwrap();
        s.set("test-provider:b", "second").unwrap();
        assert_eq!(&*s.get("test-provider:a").unwrap(), CANARY);
        assert_eq!(&*s.get("test-provider:b").unwrap(), "second");
        s.delete("test-provider:a").unwrap();
        assert_eq!(
            s.get("test-provider:a").unwrap_err(),
            BackendError::NotFound
        );
        assert_eq!(&*s.get("test-provider:b").unwrap(), "second");
        assert_eq!(
            s.delete("test-provider:a").unwrap_err(),
            BackendError::NotFound
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn missing_file_reads_as_not_found() {
        let dir = temp_dir("missing");
        let s = store(&dir, PASS);
        assert_eq!(s.get("x:y").unwrap_err(), BackendError::NotFound);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn wrong_passphrase_fails() {
        let dir = temp_dir("wrongpw");
        store(&dir, PASS).set("p:a", CANARY).unwrap();
        let err = store(&dir, "not-the-passphrase").get("p:a").unwrap_err();
        assert!(
            matches!(err, BackendError::Other(ref m) if m.contains("decryption failed")),
            "{err:?}"
        );
        // A wrong-passphrase *write* must not clobber the existing file either.
        assert!(store(&dir, "not-the-passphrase").set("p:b", "x").is_err());
        assert_eq!(&*store(&dir, PASS).get("p:a").unwrap(), CANARY);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn tampered_ciphertext_fails() {
        let dir = temp_dir("tamper-ct");
        let s = store(&dir, PASS);
        s.set("p:a", CANARY).unwrap();
        let mut bytes = fs::read(s.path()).unwrap();
        let mid = HEADER_LEN + (bytes.len() - HEADER_LEN) / 2;
        bytes[mid] ^= 0x01;
        fs::write(s.path(), &bytes).unwrap();
        assert!(s.get("p:a").is_err());
        // Tag tamper too.
        let mut bytes2 = fs::read(s.path()).unwrap();
        bytes2[mid] ^= 0x01; // restore ciphertext
        let last = bytes2.len() - 1;
        bytes2[last] ^= 0x80;
        fs::write(s.path(), &bytes2).unwrap();
        assert!(s.get("p:a").is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn tampered_header_fails_authentication() {
        let dir = temp_dir("tamper-hdr");
        let s = store(&dir, PASS);
        s.set("p:a", CANARY).unwrap();
        let good = fs::read(s.path()).unwrap();
        // Flip a salt byte, a nonce byte, and the t_cost (still in bounds: 1 -> 3).
        for idx in [20usize, 40, 15] {
            let mut bad = good.clone();
            bad[idx] = if idx == 15 { 3 } else { bad[idx] ^ 0x01 };
            fs::write(s.path(), &bad).unwrap();
            assert!(s.get("p:a").is_err(), "header byte {idx} tamper accepted");
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn canary_absent_from_file_bytes_and_header_is_versioned() {
        let dir = temp_dir("plain");
        let s = store(&dir, PASS);
        s.set("test-provider:visible-account", CANARY).unwrap();
        let bytes = fs::read(s.path()).unwrap();
        let hay = |needle: &[u8]| bytes.windows(needle.len()).any(|w| w == needle);
        assert!(!hay(CANARY.as_bytes()));
        assert!(!hay(b"AEYE-SPIKE-CANARY"));
        assert!(
            !hay(b"visible-account"),
            "account names must be encrypted too"
        );
        assert_eq!(&bytes[0..4], MAGIC);
        assert_eq!(bytes[4], FORMAT_VERSION);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn file_mode_is_0600_and_dir_0700() {
        let dir = temp_dir("mode");
        let nested = dir.join("nested").join("a-eye");
        let s = FileStore::new(nested.join("credentials.enc"), PASS, KdfParams::FAST_TEST);
        s.set("p:a", "x").unwrap();
        let fm = fs::metadata(s.path()).unwrap().permissions().mode() & 0o777;
        let dm = fs::metadata(&nested).unwrap().permissions().mode() & 0o777;
        assert_eq!(fm, 0o600);
        assert_eq!(dm, 0o700);
        // Overwrite keeps the mode, and leaves no temp file behind.
        s.set("p:b", "y").unwrap();
        assert_eq!(
            fs::metadata(s.path()).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(!nested.join("credentials.enc.tmp").exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn fresh_salt_and_nonce_on_every_write() {
        let dir = temp_dir("fresh");
        let s = store(&dir, PASS);
        s.set("p:a", "same").unwrap();
        let a = fs::read(s.path()).unwrap();
        s.set("p:a", "same").unwrap();
        let b = fs::read(s.path()).unwrap();
        assert_ne!(&a[20..36], &b[20..36], "salt reused");
        assert_ne!(&a[36..48], &b[36..48], "nonce reused");
        assert_ne!(&a[HEADER_LEN..], &b[HEADER_LEN..]);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rejects_garbage_and_hostile_headers() {
        let good = encrypt_file(&Secrets::new(), b"pw", KdfParams::FAST_TEST).unwrap();
        assert_eq!(
            decrypt_file(&good[..10], b"pw").unwrap_err(),
            FileStoreError::BadFormat("file too short")
        );

        let mut bad = good.clone();
        bad[0] = b'X';
        assert_eq!(
            decrypt_file(&bad, b"pw").unwrap_err(),
            FileStoreError::BadFormat("bad magic")
        );

        let mut bad = good.clone();
        bad[4] = 9;
        assert_eq!(
            decrypt_file(&bad, b"pw").unwrap_err(),
            FileStoreError::BadFormat("unsupported format version")
        );

        // m_cost = u32::MAX must be refused *before* any expensive KDF work.
        let mut bad = good.clone();
        bad[8..12].copy_from_slice(&u32::MAX.to_be_bytes());
        assert_eq!(
            decrypt_file(&bad, b"pw").unwrap_err(),
            FileStoreError::BadFormat("kdf parameters out of bounds")
        );

        // Empty store still round-trips.
        assert!(decrypt_file(&good, b"pw").unwrap().is_empty());
    }

    #[test]
    fn serialize_reserves_exact_capacity() {
        let mut m = Secrets::new();
        for i in 0..50 {
            m.insert(format!("p:{i}"), Zeroizing::new("x".repeat(100 + i)));
        }
        let out = serialize(&m).unwrap();
        assert_eq!(out.capacity(), out.len(), "Vec reallocated while growing");
        assert_eq!(deserialize(&out).unwrap().len(), 50);
    }

    #[test]
    fn invalid_utf8_payload_is_rejected() {
        // count=1, key "k", secret = 0xff 0xfe (invalid UTF-8)
        let mut p = Vec::new();
        p.extend_from_slice(&1u32.to_be_bytes());
        p.extend_from_slice(&1u16.to_be_bytes());
        p.push(b'k');
        p.extend_from_slice(&2u32.to_be_bytes());
        p.extend_from_slice(&[0xff, 0xfe]);
        assert_eq!(
            deserialize(&p).unwrap_err(),
            FileStoreError::BadFormat("secret not utf-8")
        );
    }
}
