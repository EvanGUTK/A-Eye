//! A-Eye spike 10: scope leases and epoch revocation (ADR008).
//!
//! A *lease* is time-bounded, scope-bounded permission to capture or overlay, granted by the
//! native core after the local user approves. An *epoch* counter lets one stop action invalidate
//! every outstanding lease at once. Dispatch code calls `LeaseBroker::check` before acting and
//! `LeaseBroker::verify` before delivering the result.
//!
//! This is a spike: nothing here is wired into part 09 or any UI, and lease ids are guessable
//! counters (see the crate README for what is and is not proven).

mod broker;
mod clock;
pub mod race;

pub use broker::{
    Action, Actions, Authorization, Denial, Epoch, GrantError, Lease, LeaseBroker, LeaseId,
    LeaseRequest, MonitorId, Scope, MAX_OUTSTANDING_LEASES, MAX_TTL,
};
pub use clock::{Clock, ManualClock, SystemClock};
