//! Concurrency evidence: user approvals racing a global stop.
//!
//! Real threads, the real clock. The scenario: several approvals, all shown under epoch E, are
//! being granted while another thread runs `revoke_all`. The invariants, which must hold however
//! the threads interleave:
//!
//! 1. Once `revoke_all` has returned, no grant that begins afterwards succeeds.
//! 2. Every lease that was granted before the revoke is dead afterwards (`STALE_EPOCH`).
//! 3. Nothing is left active, and the epoch moved by exactly one.
//!
//! The harness lives in the library so the integration tests and the evidence binary run the same
//! scenario. Which interleaving a given run hits is up to the scheduler, so the report also says
//! how many iterations had both winners and losers (the race window was actually exercised).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Duration;

use crate::{Action, Actions, Denial, GrantError, Lease, LeaseBroker, LeaseRequest, Scope};

/// Grants each thread attempts. Keep `grant_threads * GRANTS_PER_THREAD` under
/// `MAX_OUTSTANDING_LEASES` so the capacity limit cannot be what refuses a grant.
const GRANTS_PER_THREAD: usize = 4;
const TRIAL_TTL: Duration = Duration::from_secs(60);
const TRIAL_MONITOR: &str = "hm-synthetic-a";

#[derive(Debug, Default)]
pub struct RaceReport {
    pub iterations: usize,
    /// Grants that succeeded (before the revoke took effect).
    pub grants_ok: usize,
    /// Grants refused because the epoch had moved on.
    pub grants_refused_stale: usize,
    /// Grants that began after `revoke_all` returned (all of these must be refused).
    pub grants_started_after_revoke: usize,
    /// Iterations in which at least one grant won and at least one lost.
    pub mixed_iterations: usize,
    /// Empty means every invariant held.
    pub violations: Vec<String>,
}

struct GrantOutcome {
    started_after_revoke: bool,
    result: Result<Lease, GrantError>,
}

pub fn grant_vs_revoke(iterations: usize, grant_threads: usize) -> RaceReport {
    let mut report = RaceReport {
        iterations,
        ..RaceReport::default()
    };
    for iteration in 0..iterations {
        run_iteration(iteration, grant_threads, &mut report);
    }
    report
}

fn run_iteration(iteration: usize, grant_threads: usize, report: &mut RaceReport) {
    let broker = Arc::new(LeaseBroker::new());
    let scope = Scope::monitor(TRIAL_MONITOR);
    let observed_epoch = broker.epoch();
    let start = Arc::new(Barrier::new(grant_threads + 1));
    let revoke_returned = Arc::new(AtomicBool::new(false));

    let granters: Vec<_> = (0..grant_threads)
        .map(|_| {
            let broker = Arc::clone(&broker);
            let start = Arc::clone(&start);
            let revoke_returned = Arc::clone(&revoke_returned);
            let scope = scope.clone();
            thread::spawn(move || {
                start.wait();
                (0..GRANTS_PER_THREAD)
                    .map(|_| {
                        // Read before granting: if it is already true, the whole revoke_all call
                        // finished before this grant began.
                        let started_after_revoke = revoke_returned.load(Ordering::Acquire);
                        let result = broker.grant(LeaseRequest {
                            scope: scope.clone(),
                            actions: Actions::NONE.with(Action::Capture),
                            ttl: TRIAL_TTL,
                            observed_epoch,
                        });
                        GrantOutcome {
                            started_after_revoke,
                            result,
                        }
                    })
                    .collect::<Vec<_>>()
            })
        })
        .collect();

    let revoker = {
        let broker = Arc::clone(&broker);
        let revoke_returned = Arc::clone(&revoke_returned);
        thread::spawn(move || {
            start.wait();
            thread::yield_now();
            broker.revoke_all();
            revoke_returned.store(true, Ordering::Release);
        })
    };

    let mut granted = Vec::new();
    let (mut ok, mut stale) = (0, 0);
    for handle in granters {
        let Ok(outcomes) = handle.join() else {
            report
                .violations
                .push(format!("iteration {iteration}: grant thread panicked"));
            continue;
        };
        for outcome in outcomes {
            if outcome.started_after_revoke {
                report.grants_started_after_revoke += 1;
            }
            match outcome.result {
                Ok(lease) => {
                    ok += 1;
                    if outcome.started_after_revoke {
                        report.violations.push(format!(
                            "iteration {iteration}: {} granted after revoke_all returned",
                            lease.id
                        ));
                    }
                    granted.push(lease);
                }
                Err(GrantError::StaleEpoch { .. }) => stale += 1,
                Err(other) => report.violations.push(format!(
                    "iteration {iteration}: unexpected refusal: {other}"
                )),
            }
        }
    }
    if revoker.join().is_err() {
        report
            .violations
            .push(format!("iteration {iteration}: revoke thread panicked"));
    }

    for lease in &granted {
        match broker.check(lease.id, Action::Capture, &scope) {
            Err(Denial::StaleEpoch { .. }) => {}
            other => report.violations.push(format!(
                "iteration {iteration}: {} still usable after revoke_all: {other:?}",
                lease.id
            )),
        }
    }
    if broker.active_leases() != 0 {
        report.violations.push(format!(
            "iteration {iteration}: {} leases still active after revoke_all",
            broker.active_leases()
        ));
    }
    if broker.epoch().get() != observed_epoch.get() + 1 {
        report.violations.push(format!(
            "iteration {iteration}: epoch moved from {} to {}, expected exactly one bump",
            observed_epoch,
            broker.epoch()
        ));
    }

    report.grants_ok += ok;
    report.grants_refused_stale += stale;
    if ok > 0 && stale > 0 {
        report.mixed_iterations += 1;
    }
}
