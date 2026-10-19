//! Real threads and the real clock. Deterministic expiry/scope/epoch cases live in the unit tests
//! in `src/broker.rs`; these cover overlapping requests racing each other and racing a stop.
//!
//! The assertions are about invariants that hold for every interleaving, so they do not depend on
//! scheduler luck. They cannot prove the absence of a race; they exercise the window many times.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};

use spike_10_scope_leases::{
    race, Action, Actions, Denial, GrantError, LeaseBroker, LeaseRequest, Scope,
    MAX_OUTSTANDING_LEASES,
};

const CAPTURE: Actions = Actions::NONE.with(Action::Capture);

fn request(broker: &LeaseBroker, ttl: Duration) -> LeaseRequest {
    LeaseRequest {
        scope: Scope::monitor("hm-a"),
        actions: CAPTURE,
        ttl,
        observed_epoch: broker.epoch(),
    }
}

#[test]
fn a_grant_racing_a_stop_never_yields_a_live_lease() {
    const ITERATIONS: usize = 300;
    const GRANT_THREADS: usize = 8;
    let report = race::grant_vs_revoke(ITERATIONS, GRANT_THREADS);

    assert!(report.violations.is_empty(), "{:#?}", report.violations);
    assert_eq!(
        report.grants_ok + report.grants_refused_stale,
        ITERATIONS * GRANT_THREADS * 4,
        "every attempt is either granted-then-revoked or refused as stale"
    );
    eprintln!(
        "race: {} iterations, {} granted then revoked, {} refused stale, {} began after the stop, \
         {} iterations with both outcomes",
        report.iterations,
        report.grants_ok,
        report.grants_refused_stale,
        report.grants_started_after_revoke,
        report.mixed_iterations
    );
}

#[test]
fn a_check_that_begins_after_a_stop_returned_is_always_denied() {
    const ITERATIONS: usize = 200;
    const CHECK_THREADS: usize = 6;

    for iteration in 0..ITERATIONS {
        let broker = Arc::new(LeaseBroker::new());
        let lease = broker
            .grant(request(&broker, Duration::from_secs(60)))
            .expect("grant");
        let start = Arc::new(Barrier::new(CHECK_THREADS + 1));
        let stop_returned = Arc::new(AtomicBool::new(false));

        let checkers: Vec<_> = (0..CHECK_THREADS)
            .map(|_| {
                let broker = Arc::clone(&broker);
                let start = Arc::clone(&start);
                let stop_returned = Arc::clone(&stop_returned);
                thread::spawn(move || {
                    start.wait();
                    let mut violations = 0;
                    for _ in 0..200 {
                        let began_after_stop = stop_returned.load(Ordering::Acquire);
                        let allowed = broker
                            .check(lease.id, Action::Capture, &Scope::monitor("hm-a"))
                            .is_ok();
                        if began_after_stop && allowed {
                            violations += 1;
                        }
                    }
                    violations
                })
            })
            .collect();
        let stopper = {
            let broker = Arc::clone(&broker);
            let stop_returned = Arc::clone(&stop_returned);
            thread::spawn(move || {
                start.wait();
                broker.revoke_all();
                stop_returned.store(true, Ordering::Release);
            })
        };

        let violations: usize = checkers
            .into_iter()
            .map(|h| h.join().expect("checker thread"))
            .sum();
        stopper.join().expect("stopper thread");
        assert_eq!(violations, 0, "iteration {iteration}");
    }
}

#[test]
fn overlapping_requests_all_get_distinct_independent_leases() {
    const THREADS: usize = 8;
    const PER_THREAD: usize = 4;
    let broker = Arc::new(LeaseBroker::new());
    let start = Arc::new(Barrier::new(THREADS));

    let handles: Vec<_> = (0..THREADS)
        .map(|_| {
            let broker = Arc::clone(&broker);
            let start = Arc::clone(&start);
            thread::spawn(move || {
                start.wait();
                (0..PER_THREAD)
                    .map(|_| {
                        broker
                            .grant(request(&broker, Duration::from_secs(60)))
                            .expect("grant")
                    })
                    .collect::<Vec<_>>()
            })
        })
        .collect();
    let leases: Vec<_> = handles
        .into_iter()
        .flat_map(|h| h.join().expect("grant thread"))
        .collect();

    let ids: HashSet<_> = leases.iter().map(|l| l.id.to_string()).collect();
    assert_eq!(ids.len(), THREADS * PER_THREAD, "no id handed out twice");
    for lease in &leases {
        assert!(broker
            .check(lease.id, Action::Capture, &Scope::monitor("hm-a"))
            .is_ok());
    }
    assert_eq!(broker.active_leases(), THREADS * PER_THREAD);
}

#[test]
fn the_lease_cap_holds_under_contention() {
    const THREADS: usize = 8;
    const PER_THREAD: usize = 16;
    const { assert!(THREADS * PER_THREAD > MAX_OUTSTANDING_LEASES) };
    let broker = Arc::new(LeaseBroker::new());
    let start = Arc::new(Barrier::new(THREADS));

    let handles: Vec<_> = (0..THREADS)
        .map(|_| {
            let broker = Arc::clone(&broker);
            let start = Arc::clone(&start);
            thread::spawn(move || {
                start.wait();
                (0..PER_THREAD)
                    .filter(|_| {
                        broker
                            .grant(request(&broker, Duration::from_secs(60)))
                            .is_ok()
                    })
                    .count()
            })
        })
        .collect();
    let granted: usize = handles.into_iter().map(|h| h.join().expect("thread")).sum();

    assert_eq!(granted, MAX_OUTSTANDING_LEASES);
    assert_eq!(broker.active_leases(), MAX_OUTSTANDING_LEASES);
}

#[test]
fn concurrent_stops_each_advance_the_epoch_exactly_once() {
    const THREADS: usize = 16;
    let broker = Arc::new(LeaseBroker::new());
    let start = Arc::new(Barrier::new(THREADS));

    let handles: Vec<_> = (0..THREADS)
        .map(|_| {
            let broker = Arc::clone(&broker);
            let start = Arc::clone(&start);
            thread::spawn(move || {
                start.wait();
                broker.revoke_all().get()
            })
        })
        .collect();
    let mut epochs: Vec<u64> = handles
        .into_iter()
        .map(|h| h.join().expect("thread"))
        .collect();
    epochs.sort_unstable();

    assert_eq!(epochs, (1..=THREADS as u64).collect::<Vec<_>>());
    assert_eq!(broker.epoch().get(), THREADS as u64);
}

#[test]
fn an_approval_prompt_opened_before_a_stop_is_refused_even_across_threads() {
    let broker = Arc::new(LeaseBroker::new());
    let prompt = request(&broker, Duration::from_secs(60));

    let stopper = {
        let broker = Arc::clone(&broker);
        thread::spawn(move || broker.revoke_all())
    };
    stopper.join().expect("stopper");

    assert!(matches!(
        broker.grant(prompt),
        Err(GrantError::StaleEpoch { .. })
    ));
}

/// Wall time passes for real here; the unit tests cover the same boundary with a manual clock.
#[test]
fn a_lease_expires_on_the_real_clock() {
    const TTL: Duration = Duration::from_secs(1);
    let broker = LeaseBroker::new();
    let granted_at = Instant::now();
    let lease = broker.grant(request(&broker, TTL)).expect("grant");
    let target = Scope::monitor("hm-a");

    assert!(
        broker.check(lease.id, Action::Capture, &target).is_ok(),
        "usable straight after the grant"
    );
    assert!(
        granted_at.elapsed() < TTL,
        "test thread stalled past the ttl; rerun"
    );

    thread::sleep(TTL + Duration::from_millis(50));
    assert_eq!(
        broker.check(lease.id, Action::Capture, &target),
        Err(Denial::Expired)
    );
}
