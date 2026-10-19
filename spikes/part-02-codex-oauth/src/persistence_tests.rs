use super::*;
use std::cell::Cell;
use std::collections::BTreeMap;
use std::net::{Ipv4Addr, TcpListener};
use std::thread::JoinHandle;
use std::time::{Instant, UNIX_EPOCH};

use spike_01_credential_store::filestore::{FileStore, KdfParams};

use crate::flow::{self, FlowConfig};
use crate::mock::{self, MockProvider, Req, simulate_browser};
use crate::mock_plan::{PlanMock, PlanMode};
use crate::secret::Secret;
use crate::siwc::{self, PlanUsageConfig};

const ACCOUNT: &str = "synthetic-test-account";
const CLIENT_ID: &str = "oaiapp_SYNTHETIC-issued-client";
const PASSPHRASE: &str = "SYNTHETIC-test-only-passphrase";
const TEST_KDF: KdfParams = KdfParams {
    m_cost_kib: 256,
    t_cost: 1,
    p_cost: 1,
};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "a-eye-oauth-persistence-{}-{}",
            std::process::id(),
            mock::rand_token("")
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn backend(&self) -> FileStore {
        FileStore::new(self.0.join("credentials.enc"), PASSPHRASE, TEST_KDF)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("remove synthetic credential fixture");
    }
}

fn tokens() -> TokenSet {
    TokenSet {
        access_token: Secret::new("SYNTHETIC-original-access"),
        refresh_token: Some(Secret::new("SYNTHETIC-original-refresh+/=&")),
        id_token: Some(Secret::new("SYNTHETIC-original-id")),
        token_type: Some("Bearer".into()),
        scope: Some("openid offline_access chatgpt.tokens.use.direct".into()),
        expires_in: Some(3600),
        obtained_at: UNIX_EPOCH + Duration::new(1_700_000_000, 123_456_789),
    }
}

fn session(url: &str, encoding: RefreshEncoding) -> Session {
    Session::new(
        Registration {
            token_url: url.into(),
            client_id: CLIENT_ID.into(),
            resource: (encoding == RefreshEncoding::Form).then(|| siwc::RESOURCE.into()),
            encoding,
        },
        tokens(),
    )
}

fn assert_tokens_eq(actual: &TokenSet, expected: &TokenSet) {
    assert_eq!(actual.access_token, expected.access_token);
    assert_eq!(actual.refresh_token, expected.refresh_token);
    assert_eq!(actual.id_token, expected.id_token);
    assert_eq!(actual.token_type, expected.token_type);
    assert_eq!(actual.scope, expected.scope);
    assert_eq!(actual.expires_in, expected.expires_in);
    assert_eq!(actual.obtained_at, expected.obtained_at);
}

fn assert_session_eq(actual: &Session, expected: &Session) {
    assert_eq!(actual.version, expected.version);
    assert_eq!(actual.registration, expected.registration);
    assert_tokens_eq(&actual.tokens, &expected.tokens);
}

fn flow_config(mock: &MockProvider) -> FlowConfig {
    FlowConfig {
        authorize_url: mock.authorize_url(),
        token_url: mock.token_url(),
        client_id: mock.client_id().into(),
        scope: "openid profile email offline_access".into(),
        callback_port: 0,
        callback_path: "/callback".into(),
        callback_timeout: Duration::from_secs(5),
    }
}

fn plan_config(mock: &PlanMock) -> PlanUsageConfig {
    PlanUsageConfig {
        authorize_url: mock.authorize_url(),
        token_url: mock.token_url(),
        issuer: mock.issuer(),
        agent_name: "A-Eye".into(),
        host_id: "urn:uuid:11111111-1111-4111-8111-111111111111".into(),
        callback_port: 0,
        callback_timeout: Duration::from_secs(5),
    }
}

fn browser(url: &url::Url) -> JoinHandle<Result<u16, String>> {
    let url = url.to_string();
    std::thread::spawn(move || simulate_browser(&url))
}

/// One bounded loopback server; it cannot silently contact a real provider.
fn refresh_endpoint(status: &'static str, body: &'static str) -> (String, JoinHandle<Req>) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}/oauth/token", listener.local_addr().unwrap());
    let handle = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        Instant::now() < deadline,
                        "refresh fixture received no request"
                    );
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("refresh fixture accept failed: {error}"),
            }
        };
        let request = mock::read_req(&mut stream).expect("read refresh fixture request");
        mock::write_resp(&mut stream, status, "", "application/json", body);
        request
    });
    (url, handle)
}

#[test]
fn encrypted_file_roundtrip_survives_fresh_store_and_preserves_every_field() {
    let fixture = Fixture::new();
    let original = session("http://127.0.0.1:0/oauth/token", RefreshEncoding::Json);
    {
        let backend = fixture.backend();
        let store = SessionStore::new(&backend, ACCOUNT).unwrap();
        assert!(store.load().unwrap().is_none());
        store.save(&original).unwrap();
    }
    let backend = fixture.backend();
    let store = SessionStore::new(&backend, ACCOUNT).unwrap();
    assert_session_eq(&store.load().unwrap().unwrap(), &original);
    let encrypted = fs::read(fixture.0.join("credentials.enc")).unwrap();
    for secret in [
        &original.tokens.access_token,
        original.tokens.refresh_token.as_ref().unwrap(),
        original.tokens.id_token.as_ref().unwrap(),
    ] {
        assert!(
            !encrypted
                .windows(secret.expose().len())
                .any(|w| w == secret.expose().as_bytes())
        );
        assert!(!format!("{original:?} {original:#?}").contains(secret.expose()));
    }
}

#[test]
fn completed_oauth_is_saved_and_restart_resumes_without_browser_or_token_request() {
    let fixture = Fixture::new();
    let mock = MockProvider::start(CLIENT_ID).unwrap();
    let config = flow_config(&mock);
    let original = {
        let backend = fixture.backend();
        let store = SessionStore::new(&backend, ACCOUNT).unwrap();
        let mut handle = None;
        let out = flow::run_persisted(&config, &store, |url| handle = Some(browser(url))).unwrap();
        assert_eq!(handle.unwrap().join().unwrap().unwrap(), 200);
        assert_eq!(mock.token_requests(), 1);
        out
    };
    let backend = fixture.backend();
    let store = SessionStore::new(&backend, ACCOUNT).unwrap();
    let resumed =
        flow::run_persisted(&config, &store, |_| panic!("stored session opened browser")).unwrap();
    assert_session_eq(&resumed, &original);
    assert_eq!(
        mock.token_requests(),
        1,
        "fresh stored session must not exchange or refresh"
    );
}

#[test]
fn plan_sign_in_persists_issued_client_and_bad_nonce_preserves_prior_session() {
    let fixture = Fixture::new();
    let mock = PlanMock::start(PlanMode::Approve).unwrap();
    let original = {
        let backend = fixture.backend();
        let store = SessionStore::new(&backend, ACCOUNT).unwrap();
        let mut handle = None;
        let out = siwc::run_and_store(&plan_config(&mock), &store, |url| {
            handle = Some(browser(url))
        })
        .unwrap();
        assert_eq!(handle.unwrap().join().unwrap().unwrap(), 200);
        let saved = store.load().unwrap().unwrap();
        assert_eq!(saved.registration.client_id, out.issued_client_id);
        assert_ne!(saved.registration.client_id, siwc::REGISTRATION_CLIENT_ID);
        assert_eq!(saved.registration.resource.as_deref(), Some(siwc::RESOURCE));
        assert_eq!(saved.registration.encoding, RefreshEncoding::Form);
        assert_tokens_eq(&saved.tokens, &out.tokens);
        saved
    };
    let invalid = PlanMock::start(PlanMode::WrongNonce).unwrap();
    let backend = fixture.backend();
    let store = SessionStore::new(&backend, ACCOUNT).unwrap();
    let mut handle = None;
    let error = siwc::run_and_store(&plan_config(&invalid), &store, |url| {
        handle = Some(browser(url))
    })
    .err()
    .unwrap();
    assert_eq!(handle.unwrap().join().unwrap().unwrap(), 200);
    assert!(matches!(
        error,
        siwc::PlanUsageError::IdToken(siwc::IdTokenError::NonceMismatch)
    ));
    assert_session_eq(&store.load().unwrap().unwrap(), &original);
    let empty = SessionStore::new(&backend, "synthetic-invalid-nonce").unwrap();
    let mut handle = None;
    assert!(
        siwc::run_and_store(&plan_config(&invalid), &empty, |url| handle =
            Some(browser(url)))
        .is_err()
    );
    assert_eq!(handle.unwrap().join().unwrap().unwrap(), 200);
    assert!(
        empty.load().unwrap().is_none(),
        "invalid first sign-in must create no credential"
    );
}

#[test]
fn resumed_plan_session_preserves_missing_plan_usage_scope() {
    let fixture = Fixture::new();
    let mock = PlanMock::start(PlanMode::NoPlanScope).unwrap();
    {
        let backend = fixture.backend();
        let store = SessionStore::new(&backend, ACCOUNT).unwrap();
        let mut handle = None;
        let out = siwc::run_and_store(&plan_config(&mock), &store, |url| {
            handle = Some(browser(url))
        })
        .unwrap();
        assert_eq!(handle.unwrap().join().unwrap().unwrap(), 200);
        assert!(!out.plan_usage_granted);
    }
    let backend = fixture.backend();
    let store = SessionStore::new(&backend, ACCOUNT).unwrap();
    let resumed = store
        .resume(
            &mock.token_url(),
            None,
            Some(siwc::RESOURCE),
            RefreshEncoding::Form,
            SystemTime::now(),
        )
        .unwrap()
        .unwrap();
    assert!(!siwc::has_plan_usage_scope(&resumed.tokens));
    assert_eq!(mock.token_requests(), 1);
}

#[test]
fn refresh_uses_bound_json_or_form_grant_and_persists_rotated_tokens() {
    for encoding in [RefreshEncoding::Json, RefreshEncoding::Form] {
        let fixture = Fixture::new();
        let (url, handle) = refresh_endpoint(
            "200 OK",
            r#"{"access_token":"SYNTHETIC-new-access","refresh_token":"SYNTHETIC-rotated-refresh","id_token":"SYNTHETIC-new-id","scope":"openid offline_access","token_type":"Bearer","expires_in":3600}"#,
        );
        let original = session(&url, encoding);
        let backend = fixture.backend();
        let store = SessionStore::new(&backend, ACCOUNT).unwrap();
        store.save(&original).unwrap();
        let before = SystemTime::now();
        let refreshed = store
            .resume(
                &url,
                Some(CLIENT_ID),
                original.registration.resource.as_deref(),
                encoding,
                before,
            )
            .unwrap()
            .unwrap();
        let after = SystemTime::now();
        let request = handle.join().unwrap();
        assert_eq!(request.method, "POST");
        assert_eq!(request.path, "/oauth/token");
        let fields: BTreeMap<String, String> = match encoding {
            RefreshEncoding::Json => serde_json::from_slice(&request.body).unwrap(),
            RefreshEncoding::Form => url::form_urlencoded::parse(&request.body)
                .into_owned()
                .collect(),
        };
        assert_eq!(
            fields.get("grant_type").map(String::as_str),
            Some("refresh_token")
        );
        assert_eq!(fields.get("client_id").map(String::as_str), Some(CLIENT_ID));
        assert_eq!(
            fields.get("refresh_token").map(String::as_str),
            original.tokens.refresh_token.as_ref().map(Secret::expose)
        );
        assert_eq!(
            fields.get("resource").map(String::as_str),
            original.registration.resource.as_deref()
        );
        assert_eq!(
            fields.len(),
            if encoding == RefreshEncoding::Json {
                3
            } else {
                4
            }
        );
        assert_eq!(
            refreshed.tokens.access_token.expose(),
            "SYNTHETIC-new-access"
        );
        assert_eq!(
            refreshed.tokens.refresh_token.as_ref().unwrap().expose(),
            "SYNTHETIC-rotated-refresh"
        );
        assert_eq!(
            refreshed.tokens.id_token.as_ref().unwrap().expose(),
            "SYNTHETIC-new-id"
        );
        assert_eq!(
            refreshed.tokens.scope.as_deref(),
            Some("openid offline_access")
        );
        assert_eq!(refreshed.tokens.expires_in, Some(3600));
        assert!(refreshed.tokens.obtained_at >= before && refreshed.tokens.obtained_at <= after);
        let backend = fixture.backend();
        let store = SessionStore::new(&backend, ACCOUNT).unwrap();
        assert_session_eq(&store.load().unwrap().unwrap(), &refreshed);
        let loaded = store
            .resume(
                &url,
                Some(CLIENT_ID),
                original.registration.resource.as_deref(),
                encoding,
                after,
            )
            .unwrap()
            .unwrap();
        assert_session_eq(&loaded, &refreshed);
    }
}

#[test]
fn refresh_retains_omitted_grant_fields_but_does_not_reuse_access_expiry() {
    let fixture = Fixture::new();
    let (url, handle) = refresh_endpoint("200 OK", r#"{"access_token":"SYNTHETIC-new-access"}"#);
    let original = session(&url, RefreshEncoding::Json);
    let backend = fixture.backend();
    let store = SessionStore::new(&backend, ACCOUNT).unwrap();
    store.save(&original).unwrap();
    let refreshed = store
        .resume(
            &url,
            Some(CLIENT_ID),
            None,
            RefreshEncoding::Json,
            SystemTime::now(),
        )
        .unwrap()
        .unwrap();
    handle.join().unwrap();
    assert_eq!(
        refreshed.tokens.refresh_token,
        original.tokens.refresh_token
    );
    assert_eq!(refreshed.tokens.id_token, original.tokens.id_token);
    assert_eq!(refreshed.tokens.scope, original.tokens.scope);
    assert_eq!(refreshed.tokens.token_type, original.tokens.token_type);
    assert!(refreshed.tokens.expires_in.is_none());
    assert_session_eq(&store.load().unwrap().unwrap(), &refreshed);
}

#[test]
fn unknown_expiry_and_refresh_margin_require_renewal() {
    for remaining in [None, Some(0), Some(60)] {
        let fixture = Fixture::new();
        let (url, handle) = refresh_endpoint(
            "200 OK",
            r#"{"access_token":"SYNTHETIC-new-access","expires_in":3600}"#,
        );
        let mut original = session(&url, RefreshEncoding::Json);
        let now = SystemTime::now();
        original.tokens.obtained_at = now;
        original.tokens.expires_in = remaining;
        let backend = fixture.backend();
        let store = SessionStore::new(&backend, ACCOUNT).unwrap();
        store.save(&original).unwrap();
        let refreshed = store
            .resume(&url, Some(CLIENT_ID), None, RefreshEncoding::Json, now)
            .unwrap()
            .unwrap();
        handle.join().unwrap();
        assert_eq!(
            refreshed.tokens.access_token.expose(),
            "SYNTHETIC-new-access"
        );
    }
}

#[test]
fn rejected_and_malformed_refreshes_keep_original_record_and_redact_errors() {
    for (status, body, rejected) in [
        (
            "400 Bad Request",
            r#"{"error":"invalid_grant","error_description":"SYNTHETIC-original-refresh+/=&"}"#,
            true,
        ),
        (
            "400 Bad Request",
            r#"{"error":"SYNTHETIC-original-refresh+/=&","error_description":"SYNTHETIC-original-refresh+/=&"}"#,
            true,
        ),
        ("200 OK", "SYNTHETIC-original-refresh+/=&", false),
        (
            "200 OK",
            r#"{"refresh_token":"SYNTHETIC-bad-response"}"#,
            false,
        ),
        ("200 OK", r#"{"access_token":""}"#, false),
        (
            "200 OK",
            r#"{"access_token":"SYNTHETIC-new-access","refresh_token":""}"#,
            false,
        ),
    ] {
        let fixture = Fixture::new();
        let (url, handle) = refresh_endpoint(status, body);
        let original = session(&url, RefreshEncoding::Json);
        let backend = fixture.backend();
        let store = SessionStore::new(&backend, ACCOUNT).unwrap();
        store.save(&original).unwrap();
        let error = store
            .resume(
                &url,
                Some(CLIENT_ID),
                None,
                RefreshEncoding::Json,
                SystemTime::now(),
            )
            .unwrap_err();
        handle.join().unwrap();
        if rejected {
            assert!(matches!(
                &error,
                SessionError::Refresh(token::TokenError::Rejected {
                    status: 400,
                    description: None,
                    ..
                })
            ));
        } else {
            assert!(matches!(
                &error,
                SessionError::Refresh(token::TokenError::Malformed(_))
            ));
        }
        assert!(!format!("{error} {error:?}").contains("SYNTHETIC-"));
        assert_session_eq(&store.load().unwrap().unwrap(), &original);
    }
}

#[test]
fn network_failure_and_missing_refresh_token_do_not_overwrite_or_open_browser() {
    let fixture = Fixture::new();
    let backend = fixture.backend();
    let store = SessionStore::new(&backend, ACCOUNT).unwrap();
    let url = "http://127.0.0.1:0/oauth/token";
    let mut original = session(url, RefreshEncoding::Json);
    store.save(&original).unwrap();
    let error = store
        .resume(
            url,
            Some(CLIENT_ID),
            None,
            RefreshEncoding::Json,
            SystemTime::now(),
        )
        .unwrap_err();
    assert!(matches!(
        error,
        SessionError::Refresh(token::TokenError::Network(_))
    ));
    assert_session_eq(&store.load().unwrap().unwrap(), &original);
    original.tokens.refresh_token = None;
    store.save(&original).unwrap();
    let config = FlowConfig {
        authorize_url: "http://127.0.0.1:0/oauth/authorize".into(),
        token_url: url.into(),
        client_id: CLIENT_ID.into(),
        scope: "openid".into(),
        callback_port: 0,
        callback_path: "/callback".into(),
        callback_timeout: Duration::from_secs(1),
    };
    let error = flow::run_persisted(&config, &store, |_| {
        panic!("unrenewable session opened browser")
    })
    .unwrap_err();
    assert!(matches!(
        error,
        flow::FlowError::Session(SessionError::SignInRequired)
    ));
    assert_session_eq(&store.load().unwrap().unwrap(), &original);
}

#[test]
fn registration_mismatches_never_send_credentials_even_when_expired() {
    let fixture = Fixture::new();
    let mock = MockProvider::start(CLIENT_ID).unwrap();
    let url = mock.token_url();
    let backend = fixture.backend();
    let store = SessionStore::new(&backend, ACCOUNT).unwrap();
    for fresh in [false, true] {
        let mut original = session(&url, RefreshEncoding::Json);
        if fresh {
            original.tokens.obtained_at = SystemTime::now();
        }
        store.save(&original).unwrap();
        for (endpoint, client, resource, encoding) in [
            (
                format!("{url}?other-endpoint"),
                CLIENT_ID,
                None,
                RefreshEncoding::Json,
            ),
            (
                url.clone(),
                "SYNTHETIC-other-client",
                None,
                RefreshEncoding::Json,
            ),
            (
                url.clone(),
                CLIENT_ID,
                Some(siwc::RESOURCE),
                RefreshEncoding::Json,
            ),
            (url.clone(), CLIENT_ID, None, RefreshEncoding::Form),
        ] {
            assert!(matches!(
                store.resume(
                    &endpoint,
                    Some(client),
                    resource,
                    encoding,
                    SystemTime::now()
                ),
                Err(SessionError::BindingMismatch)
            ));
            assert_session_eq(&store.load().unwrap().unwrap(), &original);
            assert_eq!(mock.token_requests(), 0);
        }
    }
}

#[test]
fn corrupt_or_future_version_records_are_errors_and_remain_unchanged() {
    let fixture = Fixture::new();
    let backend = fixture.backend();
    let store = SessionStore::new(&backend, ACCOUNT).unwrap();
    let mut unsupported = session("http://127.0.0.1:0/oauth/token", RefreshEncoding::Json);
    unsupported.version = VERSION + 1;
    for record in [
        "{SYNTHETIC-invalid-secret-json".to_string(),
        serde_json::to_string(&unsupported).unwrap(),
    ] {
        backend.set(&store.account, &record).unwrap();
        let error = store.load().unwrap_err();
        assert!(matches!(error, SessionError::InvalidRecord));
        assert!(!format!("{error} {error:?}").contains("SYNTHETIC-"));
        assert_eq!(backend.get(&store.account).unwrap().as_str(), record);
    }
}

#[derive(Clone, Copy)]
enum Fault {
    Read,
    Write,
    Readback,
    Mismatch,
}

struct FaultyBackend<'a> {
    inner: &'a FileStore,
    fault: Fault,
    wrote: Cell<bool>,
}

impl CredentialBackend for FaultyBackend<'_> {
    fn name(&self) -> &'static str {
        "synthetic-fault-backend"
    }

    fn set(&self, account: &str, secret: &str) -> Result<(), BackendError> {
        if matches!(self.fault, Fault::Write) {
            return Err(BackendError::Unavailable(
                "SYNTHETIC-sensitive-backend-error".into(),
            ));
        }
        self.inner.set(account, secret)?;
        self.wrote.set(true);
        Ok(())
    }

    fn get(&self, account: &str) -> Result<Zeroizing<String>, BackendError> {
        if matches!(self.fault, Fault::Read)
            || (matches!(self.fault, Fault::Readback) && self.wrote.get())
        {
            return Err(BackendError::Unavailable(
                "SYNTHETIC-sensitive-backend-error".into(),
            ));
        }
        if matches!(self.fault, Fault::Mismatch) && self.wrote.get() {
            return Ok(Zeroizing::new("SYNTHETIC-mismatched-readback".into()));
        }
        self.inner.get(account)
    }

    fn delete(&self, account: &str) -> Result<(), BackendError> {
        self.inner.delete(account)
    }
}

#[test]
fn unavailable_store_and_failed_readback_are_propagated_without_sensitive_diagnostics() {
    for (fault, operation) in [
        (Fault::Read, "read"),
        (Fault::Write, "write"),
        (Fault::Readback, "readback"),
        (Fault::Mismatch, "readback comparison"),
    ] {
        let fixture = Fixture::new();
        let backend = fixture.backend();
        let faulty = FaultyBackend {
            inner: &backend,
            fault,
            wrote: Cell::new(false),
        };
        let store = SessionStore::new(&faulty, ACCOUNT).unwrap();
        let original = session("http://127.0.0.1:0/oauth/token", RefreshEncoding::Json);
        let error = if matches!(fault, Fault::Read) {
            store
                .resume(
                    &original.registration.token_url,
                    Some(CLIENT_ID),
                    None,
                    RefreshEncoding::Json,
                    SystemTime::now(),
                )
                .unwrap_err()
        } else {
            store.save(&original).unwrap_err()
        };
        assert!(matches!(error, SessionError::Store(op) if op == operation));
        assert!(!format!("{error} {error:?}").contains("SYNTHETIC-"));
        if matches!(fault, Fault::Read | Fault::Write) {
            assert!(
                SessionStore::new(&backend, ACCOUNT)
                    .unwrap()
                    .load()
                    .unwrap()
                    .is_none()
            );
        }
    }
}

#[test]
fn refresh_cannot_report_success_if_rotated_credential_write_or_readback_fails() {
    for fault in [Fault::Write, Fault::Readback] {
        let fixture = Fixture::new();
        let (url, handle) = refresh_endpoint(
            "200 OK",
            r#"{"access_token":"SYNTHETIC-new-access","refresh_token":"SYNTHETIC-rotated-refresh","expires_in":3600}"#,
        );
        let original = session(&url, RefreshEncoding::Json);
        let backend = fixture.backend();
        SessionStore::new(&backend, ACCOUNT)
            .unwrap()
            .save(&original)
            .unwrap();
        let faulty = FaultyBackend {
            inner: &backend,
            fault,
            wrote: Cell::new(false),
        };
        let store = SessionStore::new(&faulty, ACCOUNT).unwrap();
        let error = store
            .resume(
                &url,
                Some(CLIENT_ID),
                None,
                RefreshEncoding::Json,
                SystemTime::now(),
            )
            .unwrap_err();
        handle.join().unwrap();
        assert!(matches!(error, SessionError::Store(_)));
        let saved = SessionStore::new(&backend, ACCOUNT)
            .unwrap()
            .load()
            .unwrap()
            .unwrap();
        if matches!(fault, Fault::Write) {
            assert_session_eq(&saved, &original);
        } else {
            assert_eq!(
                saved.tokens.refresh_token.as_ref().unwrap().expose(),
                "SYNTHETIC-rotated-refresh"
            );
        }
    }
}

#[test]
fn process_lock_contends_then_releases_without_unlinking_inode() {
    let fixture = Fixture::new();
    let path = fixture.0.join("locks/oauth.lock");
    let first = lock(&path).unwrap();
    assert!(lock(&path).is_err());
    drop(first);
    let second = lock(&path).unwrap();
    assert!(path.is_file());
    assert!(lock(&path).is_err());
    drop(second);
    assert!(lock(&path).is_ok());
}
