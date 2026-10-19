//! The lease broker: grants leases, checks them at dispatch time, revokes them.
//!
//! A lease is permission to perform some `Actions` on one `Scope` until a deadline, and only while
//! the broker's epoch is still the epoch the lease was granted in. `revoke_all` bumps the epoch,
//! which kills every outstanding lease at once whatever time it has left (ADR008).
//!
//! Trust model:
//! - Only the native core calls `grant`, and only after the local user approved. A lease request
//!   arriving from an agent or MCP host is never an authorization (CONTRACTS.md).
//! - Callers present only a `LeaseId`. Scope, actions, epoch and deadline are read from the
//!   broker's own record, never from anything the caller supplies.
//! - Every failure denies. There is no path that returns an `Authorization` on error.

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use crate::clock::{Clock, SystemClock};

/// Longest lease a request may ask for, matching the part-09 overlay cap.
pub const MAX_TTL: Duration = Duration::from_secs(600);
/// Most leases the broker will hold at once, so a runaway requester cannot grow it unbounded.
pub const MAX_OUTSTANDING_LEASES: usize = 64;

/// Generation counter. Bumped by `revoke_all`; a lease is only valid in the epoch it was granted in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Epoch(u64);

impl Epoch {
    pub fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for Epoch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Opaque reference to a lease. Ids are never reused within a broker, and carry the id of the
/// broker that issued them so one broker's ids (and `Authorization`s) mean nothing to another.
/// That guards against mixing up instances, not against guessing: both parts are counters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LeaseId {
    broker: u64,
    serial: u64,
}

impl fmt::Display for LeaseId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "lease-{}.{}", self.broker, self.serial)
    }
}

impl std::str::FromStr for LeaseId {
    type Err = Denial;

    /// Parse the wire representation, not an authorization. Always call `check` afterwards.
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (broker, serial) = value
            .strip_prefix("lease-")
            .and_then(|s| s.split_once('.'))
            .ok_or(Denial::UnknownLease)?;
        let parse = |part: &str| {
            if part.is_empty()
                || !part.bytes().all(|b| b.is_ascii_digit())
                || (part.len() > 1 && part.starts_with('0'))
            {
                return Err(Denial::UnknownLease);
            }
            part.parse::<u64>().map_err(|_| Denial::UnknownLease)
        };
        Ok(Self {
            broker: parse(broker)?,
            serial: parse(serial)?,
        })
    }
}

/// Source of per-process broker ids.
static NEXT_BROKER_ID: AtomicU64 = AtomicU64::new(0);

/// Instance-scoped monitor identity (part 09 uses `hm-<hex HMONITOR>`). Opaque to the broker.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MonitorId(String);

impl MonitorId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// What a lease covers. Only whole monitors exist so far; regions are a later roadmap item.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Scope {
    Monitor(MonitorId),
}

impl Scope {
    pub fn monitor(id: impl Into<String>) -> Self {
        Self::Monitor(MonitorId::new(id))
    }

    /// True when a lease with `self` as its scope may act on `target`.
    pub fn covers(&self, target: &Scope) -> bool {
        match (self, target) {
            (Scope::Monitor(held), Scope::Monitor(wanted)) => held == wanted,
        }
    }

    fn is_empty(&self) -> bool {
        match self {
            Scope::Monitor(id) => id.as_str().is_empty(),
        }
    }
}

impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Scope::Monitor(id) => write!(f, "monitor {}", id.as_str()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Capture,
    Overlay,
}

impl Action {
    const fn bit(self) -> u8 {
        match self {
            Action::Capture => 0b01,
            Action::Overlay => 0b10,
        }
    }
}

/// A set of `Action`s.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Actions(u8);

impl Actions {
    pub const NONE: Actions = Actions(0);

    pub const fn with(self, action: Action) -> Self {
        Actions(self.0 | action.bit())
    }

    pub const fn contains(self, action: Action) -> bool {
        self.0 & action.bit() != 0
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// What the local user approved. `observed_epoch` is the epoch that was current when the approval
/// prompt was shown: if a stop happened while the prompt was open, the grant is refused.
#[derive(Clone, Debug)]
pub struct LeaseRequest {
    pub scope: Scope,
    pub actions: Actions,
    pub ttl: Duration,
    pub observed_epoch: Epoch,
}

/// A granted lease, as returned to the native core.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lease {
    pub id: LeaseId,
    pub scope: Scope,
    pub actions: Actions,
    pub epoch: Epoch,
    /// Deadline on the broker's clock (exclusive: the lease is dead at this instant).
    pub expires_at: Duration,
}

/// Proof that `check` passed. Only the broker can create one. It carries the epoch the action ran
/// under, to stamp the resulting evidence (`capture_epoch`), and goes back to `verify` before the
/// result is delivered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Authorization {
    lease: LeaseId,
    epoch: Epoch,
}

impl Authorization {
    pub fn lease_id(&self) -> LeaseId {
        self.lease
    }

    pub fn epoch(&self) -> Epoch {
        self.epoch
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GrantError {
    /// A stop (or scope switch) happened after the approval was shown. Ask again.
    StaleEpoch {
        observed: Epoch,
        current: Epoch,
    },
    EmptyScope,
    NoActions,
    ZeroTtl,
    TtlTooLong {
        requested: Duration,
        max: Duration,
    },
    TooManyLeases {
        max: usize,
    },
    /// `now + ttl` is not representable; refuse instead of granting an unbounded lease.
    ClockOverflow,
}

impl fmt::Display for GrantError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GrantError::StaleEpoch { observed, current } => write!(
                f,
                "approval was for epoch {observed} but the current epoch is {current}"
            ),
            GrantError::EmptyScope => write!(f, "scope names no monitor"),
            GrantError::NoActions => write!(f, "lease grants no actions"),
            GrantError::ZeroTtl => write!(f, "ttl must be greater than zero"),
            GrantError::TtlTooLong { requested, max } => {
                write!(f, "ttl {requested:?} exceeds the maximum {max:?}")
            }
            GrantError::TooManyLeases { max } => {
                write!(f, "already holding the maximum of {max} active leases")
            }
            GrantError::ClockOverflow => write!(f, "lease deadline overflows the clock"),
        }
    }
}

impl std::error::Error for GrantError {}

/// Why a lease was refused. Every variant is a denial.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Denial {
    /// Never issued, revoked individually, or pruned after it died.
    UnknownLease,
    /// `revoke_all` ran after this lease was granted.
    StaleEpoch {
        lease: Epoch,
        current: Epoch,
    },
    Expired,
    ScopeMismatch,
    ActionNotGranted,
}

impl Denial {
    /// The CONTRACTS.md error code this denial maps to.
    pub fn code(&self) -> &'static str {
        match self {
            Denial::UnknownLease | Denial::Expired | Denial::ActionNotGranted => "NO_ACTIVE_SCOPE",
            Denial::StaleEpoch { .. } => "STALE_EPOCH",
            Denial::ScopeMismatch => "SCOPE_MISMATCH",
        }
    }
}

impl fmt::Display for Denial {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Denial::UnknownLease => write!(f, "no such lease"),
            Denial::StaleEpoch { lease, current } => {
                write!(f, "lease is from epoch {lease}, current epoch is {current}")
            }
            Denial::Expired => write!(f, "lease expired"),
            Denial::ScopeMismatch => write!(f, "request is outside the lease scope"),
            Denial::ActionNotGranted => write!(f, "lease does not grant this action"),
        }
    }
}

impl std::error::Error for Denial {}

#[derive(Debug)]
struct Record {
    scope: Scope,
    actions: Actions,
    epoch: Epoch,
    expires_at: Duration,
}

#[derive(Debug)]
struct State {
    broker_id: u64,
    epoch: Epoch,
    next_id: u64,
    /// Latest time the clock has reported. Time seen by the broker never goes backwards, so a
    /// clock that steps back cannot make a lease live longer than its ttl.
    latest_now: Duration,
    leases: HashMap<LeaseId, Record>,
}

impl State {
    /// The lease is alive: granted in the current epoch and not past its deadline.
    fn live(&self, id: LeaseId, now: Duration) -> Result<&Record, Denial> {
        let record = self.leases.get(&id).ok_or(Denial::UnknownLease)?;
        // Revocation is reported ahead of expiry: it is the reason a caller must not retry.
        if record.epoch != self.epoch {
            return Err(Denial::StaleEpoch {
                lease: record.epoch,
                current: self.epoch,
            });
        }
        if now >= record.expires_at {
            return Err(Denial::Expired);
        }
        Ok(record)
    }

    /// Reads the clock under the lock, clamped to never move backwards.
    fn now(&mut self, clock: &impl Clock) -> Duration {
        self.latest_now = self.latest_now.max(clock.now());
        self.latest_now
    }

    fn is_active(&self, record: &Record, now: Duration) -> bool {
        record.epoch == self.epoch && now < record.expires_at
    }
}

/// Thread-safe. Share it as `Arc<LeaseBroker>`. All operations are linearizable: they run under
/// one lock, so once `revoke_all` returns, no later `check`, `verify` or `grant` can succeed for
/// a lease or approval from before it.
#[derive(Debug)]
pub struct LeaseBroker<C: Clock = SystemClock> {
    clock: C,
    state: Mutex<State>,
}

impl LeaseBroker<SystemClock> {
    pub fn new() -> Self {
        Self::with_clock(SystemClock::new())
    }
}

impl Default for LeaseBroker<SystemClock> {
    fn default() -> Self {
        Self::new()
    }
}

impl<C: Clock> LeaseBroker<C> {
    pub fn with_clock(clock: C) -> Self {
        Self {
            clock,
            state: Mutex::new(State {
                broker_id: NEXT_BROKER_ID.fetch_add(1, Ordering::Relaxed),
                epoch: Epoch(0),
                next_id: 0,
                latest_now: Duration::ZERO,
                leases: HashMap::new(),
            }),
        }
    }

    // Critical sections below only do map and integer work and cannot panic in practice (the two
    // `expect`s guard 2^64 counters and fire before the state is left half-updated), so a poisoned
    // lock still holds consistent state. Recovering matters for `revoke_all`: a stop must work
    // even if some other thread died.
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The current epoch. The UI reads this when it shows an approval prompt and puts it in
    /// `LeaseRequest::observed_epoch`.
    pub fn epoch(&self) -> Epoch {
        self.lock().epoch
    }

    /// Records a user-approved lease. Native core only.
    pub fn grant(&self, request: LeaseRequest) -> Result<Lease, GrantError> {
        if request.scope.is_empty() {
            return Err(GrantError::EmptyScope);
        }
        if request.actions.is_empty() {
            return Err(GrantError::NoActions);
        }
        if request.ttl.is_zero() {
            return Err(GrantError::ZeroTtl);
        }
        if request.ttl > MAX_TTL {
            return Err(GrantError::TtlTooLong {
                requested: request.ttl,
                max: MAX_TTL,
            });
        }

        let mut state = self.lock();
        if request.observed_epoch != state.epoch {
            return Err(GrantError::StaleEpoch {
                observed: request.observed_epoch,
                current: state.epoch,
            });
        }
        let now = state.now(&self.clock);
        let expires_at = now
            .checked_add(request.ttl)
            .ok_or(GrantError::ClockOverflow)?;

        // Dead leases are dropped here, so memory is bounded without a background thread.
        let current = state.epoch;
        state
            .leases
            .retain(|_, r| r.epoch == current && now < r.expires_at);
        if state.leases.len() >= MAX_OUTSTANDING_LEASES {
            return Err(GrantError::TooManyLeases {
                max: MAX_OUTSTANDING_LEASES,
            });
        }

        let id = LeaseId {
            broker: state.broker_id,
            serial: state.next_id,
        };
        state.next_id = state
            .next_id
            .checked_add(1)
            .expect("lease id counter exhausted after 2^64 grants");
        state.leases.insert(
            id,
            Record {
                scope: request.scope.clone(),
                actions: request.actions,
                epoch: current,
                expires_at,
            },
        );
        Ok(Lease {
            id,
            scope: request.scope,
            actions: request.actions,
            epoch: current,
            expires_at,
        })
    }

    /// Dispatch-time gate: may `action` be performed on `target` under lease `id` right now?
    pub fn check(
        &self,
        id: LeaseId,
        action: Action,
        target: &Scope,
    ) -> Result<Authorization, Denial> {
        let mut state = self.lock();
        let now = state.now(&self.clock);
        let record = state.live(id, now)?;
        if !record.scope.covers(target) {
            return Err(Denial::ScopeMismatch);
        }
        if !record.actions.contains(action) {
            return Err(Denial::ActionNotGranted);
        }
        Ok(Authorization {
            lease: id,
            epoch: record.epoch,
        })
    }

    /// Re-check an `Authorization` just before its result (a captured frame) leaves the core. Work
    /// that started under a lease must not be delivered after the lease died: a stop, or the
    /// deadline, may have passed while the capture was in flight.
    ///
    /// This closes the gap up to the moment of the call, not after it: a stop that lands between
    /// `verify` returning and the frame actually leaving the process is not caught.
    pub fn verify(&self, authorization: &Authorization) -> Result<(), Denial> {
        let mut state = self.lock();
        let now = state.now(&self.clock);
        let record = state.live(authorization.lease, now)?;
        if record.epoch != authorization.epoch {
            // Cannot happen for an Authorization this broker made; refuse rather than trust it.
            return Err(Denial::UnknownLease);
        }
        Ok(())
    }

    /// Global stop: bumps the epoch, instantly invalidating every outstanding lease and every
    /// approval prompt that is still open. Returns the new epoch.
    pub fn revoke_all(&self) -> Epoch {
        let mut state = self.lock();
        let next = state
            .epoch
            .0
            .checked_add(1)
            .expect("epoch counter exhausted after 2^64 revocations");
        state.epoch = Epoch(next);
        // Records are kept so the dead leases still report STALE_EPOCH; the next grant prunes them.
        state.epoch
    }

    /// Revokes one lease. Returns whether it was still held. Other leases are unaffected.
    pub fn revoke(&self, id: LeaseId) -> bool {
        self.lock().leases.remove(&id).is_some()
    }

    /// Leases that would pass `check` right now. Expired and revoked ones are not counted, so
    /// this is safe to drive a "capture is live" indicator.
    pub fn active_leases(&self) -> usize {
        let mut state = self.lock();
        let now = state.now(&self.clock);
        state
            .leases
            .values()
            .filter(|r| state.is_active(r, now))
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::ManualClock;
    use std::sync::Arc;

    const CAPTURE: Actions = Actions::NONE.with(Action::Capture);
    const BOTH: Actions = CAPTURE.with(Action::Overlay);

    #[test]
    fn wire_lease_ids_round_trip_but_still_require_broker_authorization() {
        let broker = LeaseBroker::new();
        let lease = grant(&broker, "hm-a", CAPTURE, secs(5));
        let parsed = lease.id.to_string().parse::<LeaseId>().unwrap();
        assert_eq!(parsed, lease.id);
        assert!(broker
            .check(parsed, Action::Capture, &Scope::monitor("hm-a"))
            .is_ok());
        let other = LeaseBroker::new();
        assert_eq!(
            other.check(parsed, Action::Capture, &Scope::monitor("hm-a")),
            Err(Denial::UnknownLease)
        );
        let maximal = "lease-18446744073709551615.18446744073709551615";
        assert_eq!(maximal.parse::<LeaseId>().unwrap().to_string(), maximal);
    }

    #[test]
    fn malformed_wire_lease_ids_are_denied() {
        for value in [
            "",
            "lease-",
            "lease-1",
            "lease-1.",
            "lease-.1",
            "lease-1.2.3",
            "lease-01.2",
            "lease-1.02",
            "lease-+1.2",
            "lease-1.-2",
            "lease-1. 2",
            "lease-1.2 ",
            " lease-1.2",
            "lease-1.2\0",
            "lease-１.2",
            "lease-18446744073709551616.0",
            "lease-0.18446744073709551616",
        ] {
            assert_eq!(
                value.parse::<LeaseId>(),
                Err(Denial::UnknownLease),
                "{value:?}"
            );
        }
    }

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    fn setup() -> (Arc<ManualClock>, LeaseBroker<Arc<ManualClock>>) {
        let clock = Arc::new(ManualClock::new());
        let broker = LeaseBroker::with_clock(Arc::clone(&clock));
        (clock, broker)
    }

    fn request<C: Clock>(
        broker: &LeaseBroker<C>,
        monitor: &str,
        actions: Actions,
        ttl: Duration,
    ) -> LeaseRequest {
        LeaseRequest {
            scope: Scope::monitor(monitor),
            actions,
            ttl,
            observed_epoch: broker.epoch(),
        }
    }

    fn grant<C: Clock>(
        broker: &LeaseBroker<C>,
        monitor: &str,
        actions: Actions,
        ttl: Duration,
    ) -> Lease {
        broker
            .grant(request(broker, monitor, actions, ttl))
            .expect("grant should succeed")
    }

    // (1) expiry

    #[test]
    fn lease_is_valid_until_its_ttl_then_expires_on_its_own() {
        let (clock, broker) = setup();
        let lease = grant(&broker, "hm-a", CAPTURE, secs(5));
        let target = Scope::monitor("hm-a");

        assert!(broker.check(lease.id, Action::Capture, &target).is_ok());
        clock.advance(Duration::from_millis(4999));
        assert!(broker.check(lease.id, Action::Capture, &target).is_ok());
        assert_eq!(broker.active_leases(), 1);

        clock.advance(Duration::from_millis(1));
        assert_eq!(
            broker.check(lease.id, Action::Capture, &target),
            Err(Denial::Expired),
            "the deadline itself is already dead"
        );
        assert_eq!(broker.active_leases(), 0);
        clock.advance(secs(3600));
        assert_eq!(
            broker.check(lease.id, Action::Capture, &target),
            Err(Denial::Expired)
        );
    }

    #[test]
    fn expiry_is_per_lease() {
        let (clock, broker) = setup();
        let short = grant(&broker, "hm-a", CAPTURE, secs(2));
        let long = grant(&broker, "hm-a", CAPTURE, secs(10));
        let target = Scope::monitor("hm-a");

        clock.advance(secs(3));
        assert_eq!(
            broker.check(short.id, Action::Capture, &target),
            Err(Denial::Expired)
        );
        assert!(broker.check(long.id, Action::Capture, &target).is_ok());
    }

    #[test]
    fn verify_refuses_delivery_after_the_lease_expired_mid_flight() {
        let (clock, broker) = setup();
        let lease = grant(&broker, "hm-a", CAPTURE, secs(5));
        let auth = broker
            .check(lease.id, Action::Capture, &Scope::monitor("hm-a"))
            .unwrap();

        assert_eq!(broker.verify(&auth), Ok(()));
        clock.advance(secs(5));
        assert_eq!(broker.verify(&auth), Err(Denial::Expired));
    }

    // (2) epoch revocation

    #[test]
    fn revoke_all_kills_every_unexpired_lease_at_once() {
        let (clock, broker) = setup();
        let a = grant(&broker, "hm-a", CAPTURE, secs(600));
        let b = grant(&broker, "hm-b", BOTH, secs(600));
        clock.advance(secs(1));
        assert_eq!(broker.active_leases(), 2);

        let before = broker.epoch();
        let after = broker.revoke_all();
        assert_eq!(after.get(), before.get() + 1);

        for (lease, monitor) in [(&a, "hm-a"), (&b, "hm-b")] {
            let denial = broker
                .check(lease.id, Action::Capture, &Scope::monitor(monitor))
                .unwrap_err();
            assert_eq!(
                denial,
                Denial::StaleEpoch {
                    lease: before,
                    current: after
                }
            );
            assert_eq!(denial.code(), "STALE_EPOCH");
        }
        assert_eq!(broker.active_leases(), 0);
    }

    #[test]
    fn revoke_all_invalidates_an_authorization_already_issued() {
        let (_clock, broker) = setup();
        let lease = grant(&broker, "hm-a", CAPTURE, secs(60));
        let auth = broker
            .check(lease.id, Action::Capture, &Scope::monitor("hm-a"))
            .unwrap();
        assert_eq!(auth.epoch(), lease.epoch);
        assert_eq!(auth.lease_id(), lease.id);

        broker.revoke_all();
        assert!(matches!(
            broker.verify(&auth),
            Err(Denial::StaleEpoch { .. })
        ));
    }

    #[test]
    fn leases_granted_after_a_revoke_work_and_old_ones_stay_dead() {
        let (_clock, broker) = setup();
        let old = grant(&broker, "hm-a", CAPTURE, secs(60));
        let epoch = broker.revoke_all();
        let fresh = grant(&broker, "hm-a", CAPTURE, secs(60));
        let target = Scope::monitor("hm-a");

        assert_eq!(fresh.epoch, epoch);
        assert!(broker.check(fresh.id, Action::Capture, &target).is_ok());
        assert!(broker.check(old.id, Action::Capture, &target).is_err());
        assert_ne!(old.id, fresh.id, "ids are never reused");
    }

    #[test]
    fn repeated_revokes_each_advance_the_epoch_by_one() {
        let (_clock, broker) = setup();
        assert_eq!(broker.epoch().get(), 0);
        for expected in 1..=5 {
            assert_eq!(broker.revoke_all().get(), expected);
        }
    }

    #[test]
    fn staleness_is_reported_ahead_of_expiry() {
        let (clock, broker) = setup();
        let lease = grant(&broker, "hm-a", CAPTURE, secs(1));
        broker.revoke_all();
        clock.advance(secs(10));
        assert_eq!(
            broker
                .check(lease.id, Action::Capture, &Scope::monitor("hm-a"))
                .unwrap_err()
                .code(),
            "STALE_EPOCH"
        );
    }

    // an approval that raced a stop

    #[test]
    fn approval_shown_before_a_stop_cannot_be_granted_after_it() {
        let (_clock, broker) = setup();
        let shown_to_user = request(&broker, "hm-a", CAPTURE, secs(30));
        broker.revoke_all();

        assert_eq!(
            broker.grant(shown_to_user),
            Err(GrantError::StaleEpoch {
                observed: Epoch(0),
                current: Epoch(1)
            })
        );
        assert_eq!(broker.active_leases(), 0);
    }

    #[test]
    fn a_future_epoch_is_not_accepted_either() {
        let (_clock, broker) = setup();
        let mut forged = request(&broker, "hm-a", CAPTURE, secs(30));
        forged.observed_epoch = Epoch(7);
        assert!(matches!(
            broker.grant(forged),
            Err(GrantError::StaleEpoch { .. })
        ));
    }

    // (3) scope

    #[test]
    fn lease_for_monitor_a_is_refused_for_monitor_b() {
        let (_clock, broker) = setup();
        let lease = grant(&broker, "hm-a", CAPTURE, secs(60));

        assert!(broker
            .check(lease.id, Action::Capture, &Scope::monitor("hm-a"))
            .is_ok());
        let denial = broker
            .check(lease.id, Action::Capture, &Scope::monitor("hm-b"))
            .unwrap_err();
        assert_eq!(denial, Denial::ScopeMismatch);
        assert_eq!(denial.code(), "SCOPE_MISMATCH");
    }

    #[test]
    fn near_miss_monitor_ids_do_not_match() {
        let (_clock, broker) = setup();
        let lease = grant(&broker, "hm-a", CAPTURE, secs(60));
        for other in ["hm-A", "hm-a ", "hm-", "", "hm-a\0", "HM-A"] {
            assert_eq!(
                broker.check(lease.id, Action::Capture, &Scope::monitor(other)),
                Err(Denial::ScopeMismatch),
                "{other:?}"
            );
        }
    }

    #[test]
    fn lease_covers_only_the_actions_it_was_granted() {
        let (_clock, broker) = setup();
        let lease = grant(&broker, "hm-a", CAPTURE, secs(60));
        let target = Scope::monitor("hm-a");
        assert_eq!(
            broker.check(lease.id, Action::Overlay, &target),
            Err(Denial::ActionNotGranted)
        );
        let both = grant(&broker, "hm-a", BOTH, secs(60));
        assert!(broker.check(both.id, Action::Overlay, &target).is_ok());
    }

    // (4) overlapping leases

    #[test]
    fn overlapping_leases_on_one_monitor_are_independent() {
        let (_clock, broker) = setup();
        let first = grant(&broker, "hm-a", CAPTURE, secs(60));
        let second = grant(&broker, "hm-a", CAPTURE, secs(60));
        let target = Scope::monitor("hm-a");
        assert_ne!(first.id, second.id);

        assert!(broker.revoke(first.id));
        assert!(!broker.revoke(first.id), "second revoke finds nothing");
        assert_eq!(
            broker.check(first.id, Action::Capture, &target),
            Err(Denial::UnknownLease)
        );
        assert!(broker.check(second.id, Action::Capture, &target).is_ok());
    }

    #[test]
    fn a_revoked_lease_id_is_never_reissued() {
        let (_clock, broker) = setup();
        let first = grant(&broker, "hm-a", CAPTURE, secs(60));
        broker.revoke(first.id);
        let ids: Vec<_> = (0..10)
            .map(|_| grant(&broker, "hm-a", CAPTURE, secs(60)).id)
            .collect();
        assert!(!ids.contains(&first.id));
    }

    #[test]
    fn unknown_lease_ids_are_denied() {
        let (_clock, broker) = setup();
        assert_eq!(
            broker.check(
                LeaseId {
                    broker: u64::MAX,
                    serial: 0
                },
                Action::Capture,
                &Scope::monitor("hm-a")
            ),
            Err(Denial::UnknownLease)
        );
        assert_eq!(
            broker.check(
                LeaseId {
                    broker: u64::MAX,
                    serial: u64::MAX
                },
                Action::Capture,
                &Scope::monitor("hm-a")
            ),
            Err(Denial::UnknownLease)
        );
    }

    // request validation and bounds

    #[test]
    fn invalid_requests_are_refused() {
        let (_clock, broker) = setup();
        assert_eq!(
            broker.grant(request(&broker, "", CAPTURE, secs(1))),
            Err(GrantError::EmptyScope)
        );
        assert_eq!(
            broker.grant(request(&broker, "hm-a", Actions::NONE, secs(1))),
            Err(GrantError::NoActions)
        );
        assert_eq!(
            broker.grant(request(&broker, "hm-a", CAPTURE, Duration::ZERO)),
            Err(GrantError::ZeroTtl)
        );
        assert_eq!(
            broker.grant(request(&broker, "hm-a", CAPTURE, MAX_TTL + secs(1))),
            Err(GrantError::TtlTooLong {
                requested: MAX_TTL + secs(1),
                max: MAX_TTL
            })
        );
        assert!(broker
            .grant(request(&broker, "hm-a", CAPTURE, MAX_TTL))
            .is_ok());
    }

    #[test]
    fn outstanding_leases_are_capped_and_expired_ones_free_a_slot() {
        let (clock, broker) = setup();
        for _ in 0..MAX_OUTSTANDING_LEASES {
            grant(&broker, "hm-a", CAPTURE, secs(10));
        }
        assert_eq!(
            broker.grant(request(&broker, "hm-a", CAPTURE, secs(10))),
            Err(GrantError::TooManyLeases {
                max: MAX_OUTSTANDING_LEASES
            })
        );
        clock.advance(secs(10));
        assert!(broker
            .grant(request(&broker, "hm-a", CAPTURE, secs(10)))
            .is_ok());
        assert_eq!(broker.active_leases(), 1);
    }

    #[test]
    fn a_deadline_that_overflows_the_clock_is_refused_not_unbounded() {
        struct EndOfTime;
        impl Clock for EndOfTime {
            fn now(&self) -> Duration {
                Duration::MAX
            }
        }
        let broker = LeaseBroker::with_clock(EndOfTime);
        assert_eq!(
            broker.grant(request(&broker, "hm-a", CAPTURE, secs(1))),
            Err(GrantError::ClockOverflow)
        );
    }

    #[test]
    fn once_pruned_a_stale_lease_reads_as_unknown_not_stale() {
        // Documents the trade-off in the README: a grant after a stop drops the dead records.
        let (_clock, broker) = setup();
        let old = grant(&broker, "hm-a", CAPTURE, secs(60));
        broker.revoke_all();
        let target = Scope::monitor("hm-a");
        assert_eq!(
            broker
                .check(old.id, Action::Capture, &target)
                .unwrap_err()
                .code(),
            "STALE_EPOCH"
        );

        grant(&broker, "hm-a", CAPTURE, secs(60));
        assert_eq!(
            broker.check(old.id, Action::Capture, &target),
            Err(Denial::UnknownLease)
        );
    }

    #[test]
    fn another_brokers_lease_ids_and_authorizations_mean_nothing() {
        let (_c1, one) = setup();
        let (_c2, two) = setup();
        let a = grant(&one, "hm-a", CAPTURE, secs(60));
        let b = grant(&two, "hm-a", CAPTURE, secs(60));
        let target = Scope::monitor("hm-a");
        assert_ne!(a.id, b.id);

        assert_eq!(
            two.check(a.id, Action::Capture, &target),
            Err(Denial::UnknownLease)
        );
        let auth = one.check(a.id, Action::Capture, &target).unwrap();
        assert_eq!(two.verify(&auth), Err(Denial::UnknownLease));
    }

    #[test]
    fn a_clock_that_steps_back_cannot_revive_an_expired_lease() {
        struct Scripted(Mutex<Vec<Duration>>);
        impl Clock for Scripted {
            fn now(&self) -> Duration {
                let mut readings = self.0.lock().unwrap();
                if readings.len() > 1 {
                    readings.remove(0)
                } else {
                    readings[0]
                }
            }
        }
        // Granted at 10 s with a 5 s ttl (deadline 15 s). The clock reads 16 s, then steps back
        // to 12 s, which on its own would be inside the ttl again.
        let clock = Scripted(Mutex::new(vec![secs(10), secs(16), secs(12)]));
        let broker = LeaseBroker::with_clock(clock);
        let lease = broker
            .grant(request(&broker, "hm-a", CAPTURE, secs(5)))
            .unwrap();
        let target = Scope::monitor("hm-a");

        assert_eq!(
            broker.check(lease.id, Action::Capture, &target),
            Err(Denial::Expired)
        );
        assert_eq!(
            broker.check(lease.id, Action::Capture, &target),
            Err(Denial::Expired),
            "an earlier reading must not bring the lease back"
        );
    }

    #[test]
    fn revoke_all_still_works_after_another_thread_panicked_holding_the_lock() {
        let broker = Arc::new(LeaseBroker::new());
        let lease = grant(&broker, "hm-a", CAPTURE, secs(60));
        let poisoner = Arc::clone(&broker);
        let _ = std::thread::spawn(move || {
            let _guard = poisoner.state.lock().unwrap();
            panic!("simulated crash while holding the broker lock");
        })
        .join();
        assert!(broker.state.is_poisoned());

        broker.revoke_all();
        assert!(broker
            .check(lease.id, Action::Capture, &Scope::monitor("hm-a"))
            .is_err());
    }
}
