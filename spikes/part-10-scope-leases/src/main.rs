//! Spike 10 evidence runner: exercises the lease broker against the real monotonic clock and real
//! threads and prints a PASS/FAIL record. Exits non-zero if any check fails.
//!
//! Usage: spike-10-scope-leases
//!
//! Synthetic monitor ids only; nothing is captured and no window is drawn.

use std::process::ExitCode;
use std::thread;
use std::time::{Duration, Instant};

use spike_10_scope_leases::{
    race, Action, Actions, Denial, GrantError, LeaseBroker, LeaseRequest, Scope,
    MAX_OUTSTANDING_LEASES, MAX_TTL,
};

const CAPTURE: Actions = Actions::NONE.with(Action::Capture);
const CAPTURE_AND_OVERLAY: Actions = CAPTURE.with(Action::Overlay);
const EXPIRY_TTL: Duration = Duration::from_millis(300);
const RACE_ITERATIONS: usize = 500;
const RACE_GRANT_THREADS: usize = 8;

#[derive(Default)]
struct Record {
    failed: usize,
}

impl Record {
    fn check(&mut self, id: &str, what: &str, ok: bool, detail: String) {
        if !ok {
            self.failed += 1;
        }
        println!(
            "[{}] {id} {what}: {detail}",
            if ok { "PASS" } else { "FAIL" }
        );
    }
}

fn request(broker: &LeaseBroker, monitor: &str, actions: Actions, ttl: Duration) -> LeaseRequest {
    LeaseRequest {
        scope: Scope::monitor(monitor),
        actions,
        ttl,
        observed_epoch: broker.epoch(),
    }
}

fn code<T>(result: &Result<T, Denial>) -> &'static str {
    match result {
        Ok(_) => "ALLOWED",
        Err(denial) => denial.code(),
    }
}

fn expiry(record: &mut Record) {
    let broker = LeaseBroker::new();
    let target = Scope::monitor("hm-a");
    let start = Instant::now();
    let lease = broker
        .grant(request(&broker, "hm-a", CAPTURE, EXPIRY_TTL))
        .expect("a 300 ms lease is within limits");

    let early = broker.check(lease.id, Action::Capture, &target);
    let early_at = start.elapsed();
    thread::sleep(EXPIRY_TTL + Duration::from_millis(50));
    let late = broker.check(lease.id, Action::Capture, &target);
    let late_at = start.elapsed();

    record.check(
        "L1",
        "lease expires on its own after its ttl",
        early.is_ok() && early_at < EXPIRY_TTL && late == Err(Denial::Expired),
        format!(
            "ttl {EXPIRY_TTL:?}: {} at +{early_at:?}, {} at +{late_at:?}; active leases now {}",
            code(&early),
            late.as_ref()
                .map_or_else(|d| d.to_string(), |_| "ALLOWED".into()),
            broker.active_leases()
        ),
    );
}

fn epoch_revocation(record: &mut Record) {
    let broker = LeaseBroker::new();
    let target = Scope::monitor("hm-a");
    let capture = broker
        .grant(request(&broker, "hm-a", CAPTURE, MAX_TTL))
        .expect("grant");
    let both = broker
        .grant(request(&broker, "hm-a", CAPTURE_AND_OVERLAY, MAX_TTL))
        .expect("grant");
    let in_flight = broker
        .check(capture.id, Action::Capture, &target)
        .expect("fresh lease passes");
    let live_before = broker.active_leases();
    let epoch_before = broker.epoch();

    let stop = Instant::now();
    let epoch_after = broker.revoke_all();
    let stop_took = stop.elapsed();

    let a = broker.check(capture.id, Action::Capture, &target);
    let b = broker.check(both.id, Action::Overlay, &target);
    let delivery = broker.verify(&in_flight);
    record.check(
        "R1",
        "revoke_all kills every outstanding lease before its ttl",
        live_before == 2
            && a.as_ref().err().map(Denial::code) == Some("STALE_EPOCH")
            && b.as_ref().err().map(Denial::code) == Some("STALE_EPOCH")
            && broker.active_leases() == 0,
        format!(
            "2 leases with ttl {MAX_TTL:?} (none expired); epoch {epoch_before} -> {epoch_after} in \
             {stop_took:?}; checks now {} / {}; active leases {live_before} -> {}",
            code(&a),
            code(&b),
            broker.active_leases()
        ),
    );
    record.check(
        "R2",
        "a capture in flight is not delivered after a stop",
        delivery.as_ref().err().map(Denial::code) == Some("STALE_EPOCH"),
        format!(
            "check passed, then revoke_all, then verify before delivery: {}",
            delivery.as_ref().map_or_else(|d| d.code(), |()| "ALLOWED")
        ),
    );

    let prompt = request(&broker, "hm-a", CAPTURE, Duration::from_secs(30));
    broker.revoke_all();
    let late_grant = broker.grant(prompt);
    record.check(
        "R3",
        "an approval prompt opened before a stop cannot be granted after it",
        matches!(late_grant, Err(GrantError::StaleEpoch { .. })),
        format!("grant after stop: {late_grant:?}"),
    );
}

fn scope_and_action(record: &mut Record) {
    let broker = LeaseBroker::new();
    let lease = broker
        .grant(request(&broker, "hm-a", CAPTURE, Duration::from_secs(60)))
        .expect("grant");
    let on_a = broker.check(lease.id, Action::Capture, &Scope::monitor("hm-a"));
    let on_b = broker.check(lease.id, Action::Capture, &Scope::monitor("hm-b"));
    record.check(
        "S1",
        "lease for monitor A is refused for monitor B",
        on_a.is_ok() && on_b == Err(Denial::ScopeMismatch),
        format!("hm-a: {}, hm-b: {}", code(&on_a), code(&on_b)),
    );

    let overlay = broker.check(lease.id, Action::Overlay, &Scope::monitor("hm-a"));
    record.check(
        "S2",
        "a capture-only lease is refused for overlay",
        overlay == Err(Denial::ActionNotGranted),
        format!("overlay on hm-a: {}", code(&overlay)),
    );
}

fn overlapping(record: &mut Record) {
    let report = race::grant_vs_revoke(RACE_ITERATIONS, RACE_GRANT_THREADS);
    record.check(
        "C1",
        "grants racing a stop never leave a live lease",
        report.violations.is_empty(),
        format!(
            "{} iterations x {RACE_GRANT_THREADS} threads: {} granted then revoked, {} refused stale \
             ({} began after the stop returned, all refused); {} iterations had both outcomes; \
             violations {}",
            report.iterations,
            report.grants_ok,
            report.grants_refused_stale,
            report.grants_started_after_revoke,
            report.mixed_iterations,
            report.violations.len()
        ),
    );
    for violation in report.violations.iter().take(5) {
        println!("       {violation}");
    }

    let broker = LeaseBroker::new();
    let granted = (0..MAX_OUTSTANDING_LEASES + 8)
        .filter(|_| {
            broker
                .grant(request(&broker, "hm-a", CAPTURE, Duration::from_secs(60)))
                .is_ok()
        })
        .count();
    record.check(
        "C2",
        "outstanding leases are capped",
        granted == MAX_OUTSTANDING_LEASES,
        format!(
            "{} attempts, {granted} granted (cap {MAX_OUTSTANDING_LEASES})",
            MAX_OUTSTANDING_LEASES + 8
        ),
    );
}

fn main() -> ExitCode {
    println!(
        "spike 10: scope leases + epoch revocation (std only, monotonic clock, synthetic monitors)"
    );
    let mut record = Record::default();
    expiry(&mut record);
    epoch_revocation(&mut record);
    scope_and_action(&mut record);
    overlapping(&mut record);

    println!();
    println!(
        "Not proven by this run: wiring into part 09, lease-id unguessability, behaviour across"
    );
    println!(
        "system suspend/resume, a user-facing approval or stop control, persistence across restart."
    );
    if record.failed == 0 {
        println!("all checks passed");
        ExitCode::SUCCESS
    } else {
        println!("{} check(s) FAILED", record.failed);
        ExitCode::FAILURE
    }
}
