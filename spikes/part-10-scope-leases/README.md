# Spike 10: scope leases + epoch revocation

A standalone, std-only Rust library (plus an evidence binary) for the consent primitive in ADR008
(`../../docs/decisions/README.md`). **It is not wired into anything yet**: part 09's
`--allow-capture` flag is untouched and still the only gate there.

Issue: #38. Status and what is unproven: see the table in `../README.md` and the end of this file.

## What it is

- A **lease** is permission for some `Actions` (`Capture`, `Overlay`) on one `Scope` (a whole
  monitor, by its instance-scoped id) until a deadline. The native core grants it after the local
  user approves; an agent asking for one is never an authorization (`CONTRACTS.md`).
- An **epoch** is a counter. A lease is only valid in the epoch it was granted in, so
  `LeaseBroker::revoke_all()` (the global stop) invalidates every outstanding lease at once, whatever
  TTL they have left.
- Callers hold only a `LeaseId`. Scope, actions, epoch and deadline are read from the broker's own
  record, never from what the caller says.

| Call | Does |
|---|---|
| `epoch()` | Current epoch. The approval prompt reads it and puts it in `LeaseRequest::observed_epoch`. |
| `grant(LeaseRequest)` | Records an approved lease. Refused if the epoch moved since the prompt was shown, TTL is 0 or over 600 s, scope or actions are empty, or 64 leases are already active. |
| `check(id, action, &scope)` | Dispatch-time gate. Returns an `Authorization` or a `Denial` carrying the `CONTRACTS.md` code (`NO_ACTIVE_SCOPE`, `SCOPE_MISMATCH`, `STALE_EPOCH`). |
| `verify(&authorization)` | Re-check just before the result (a captured frame) is delivered, so work in flight when a stop or the deadline hits is dropped. |
| `revoke_all()` | Global stop. Bumps the epoch, returns the new one. |
| `revoke(id)` | Revokes one lease; others are unaffected. |
| `active_leases()` | Count that would pass `check` now; expired and revoked ones are not counted. |

Expiry uses a monotonic clock (`std::time::Instant`), not wall-clock time, behind a `Clock` trait so
tests can move time by hand. The deadline is exclusive: a lease is dead *at* its expiry instant.

## Build and run

No dependencies. Works on WSL cargo and on native Windows cargo.

```
cd spikes/part-10-scope-leases
cargo test                 # 20 unit tests (manual clock) + 7 threaded/real-clock tests (~1 s)
cargo run                  # PASS/FAIL record against the real clock; exits non-zero on failure
```

Native Windows, over the UNC path (see `../README.md`, "Platform pivot"):

```powershell
$env:CARGO_TARGET_DIR = "$env:TEMP\aeye10-target"
cd \\wsl$\Ubuntu\home\usaas\projects\code\A-Eye-worktrees\scope-leases\spikes\part-10-scope-leases
cargo test; cargo run
```

## How part 09 could adopt it (design only, not implemented)

```
aeye_capture_monitor(monitor_id, lease_id):
    auth  = broker.check(lease_id, Action::Capture, &Scope::monitor(monitor_id))?  // denial -> tool error with its code
    frame = backend.capture_monitor(monitor_id)
    broker.verify(&auth)?                       // a stop during the capture drops the frame
    stamp the observation with auth.epoch()     // the envelope's capture_epoch
```

`grant` and `revoke_all` must be reachable only from native, local code: an approval control for
`grant`, and the existing Ctrl+Alt+Q handler (and a stop button) for `revoke_all`. They must not be
MCP tools. That wiring does not exist.

## Decisions

- **The approval carries the epoch it was shown under.** This is how "a revoked lease cannot be
  granted twice" is enforced: if the user hits stop while a prompt is open, the late approval is
  refused (`GrantError::StaleEpoch`) instead of minting a live lease in the new epoch.
- **Revocation is reported ahead of expiry.** A stale-epoch lease says `STALE_EPOCH` even if it has
  also expired, because that is the "do not retry" answer.
- **Individual revoke removes the record** (the id then reads `NO_ACTIVE_SCOPE`); ids are a
  counter and are never reused, so a revoked id cannot come back.
- **Dead records are pruned at the next `grant`**, not by a background thread, so memory is bounded
  (64 active leases) without one. After pruning, a stale lease reads `NO_ACTIVE_SCOPE` rather than
  `STALE_EPOCH`; both are denials.
- **One mutex, linearizable.** Every operation runs under one lock, so once `revoke_all` returns, no
  later `check`, `verify` or `grant` from before it can succeed. A poisoned lock is recovered
  rather than propagated, so a stop still works if another thread panicked.
- **No dependencies, no hand-rolled MCP types.** The primitive does not know about MCP or Windows.

## Evidence

Tested on this machine, with WSL cargo 1.93.1 and native Windows cargo 1.99.0 (same results):

- 20 unit tests on a manual clock: expiry at exactly the deadline (4.999 s allowed, 5.000 s denied),
  per-lease expiry, `revoke_all` killing unexpired leases and in-flight `Authorization`s, epoch
  advancing by one per stop, approval refused after a stop (and for a future epoch), monitor A lease
  refused for B and for near-miss ids (`hm-A`, trailing space, empty, NUL), action not granted,
  overlapping leases independent, revoked ids never reissued, request validation, the 64-lease cap
  and slot reuse after expiry, a clock overflow refused, `revoke_all` working after a panicked thread
  poisoned the lock.
- 7 integration tests with real threads: 300 iterations x 8 threads of grants racing `revoke_all`
  (no grant that began after the stop returned succeeded; every lease granted before it was dead
  afterwards; epoch moved by exactly one); 200 iterations x 6 threads of `check` racing a stop; 32
  concurrent grants all distinct and valid; the cap holding at exactly 64 under 128 concurrent
  attempts; 16 concurrent stops yielding epochs 1..=16 once each; a 1 s lease expiring on the real
  clock.
- `cargo run` on both machines: 8 checks (L1 expiry, R1-R3 revocation, S1-S2 scope/action, C1-C2
  concurrency) all PASS. The race check ran 500 iterations x 8 threads; the interleaving was
  mixed (some grants won, some lost) in 461 of 500 iterations on WSL and 153 of 500 on Windows, so
  the race window was exercised but how often depends on the scheduler.
- Mutation check by hand: removing the epoch comparison, making expiry inclusive, removing expiry,
  removing the scope check, ignoring `observed_epoch` in `grant`, and not bumping the epoch in
  `revoke_all` were each caught by failing tests. The threaded race tests also caught the
  `observed_epoch` mutation.
- Integration tests run 20 times in a row on WSL: 0 failures. `cargo clippy --all-targets -D
  warnings` and `cargo fmt --check` clean on both toolchains.

## Not proven

- **Not wired in.** Part 09 does not call this; no MCP tool takes a lease; no approval UI or stop
  control exists. `revoke_all` is a function, not a hotkey.
- **Lease ids are guessable counters and not bound to a client or session.** Anyone who can submit a
  `LeaseId` to the broker can use that lease. A real design needs an unguessable token bound to the
  session it was granted for.
- **In-flight work is dropped at delivery, not interrupted.** `verify` stops a stale frame from
  being returned; it cannot abort a capture already running, and nothing downstream of delivery
  honours the epoch yet.
- **Suspend/resume.** TTL is measured with `std::time::Instant`. Whether time asleep counts differs
  by OS (Linux `CLOCK_MONOTONIC` does not include suspend); not tested on either.
- **Scope is whole monitors only**, matched by id string. Region or focused-window scopes, and
  `DISPLAY_CHANGED` (a monitor id that stops meaning the same display), are not handled.
- **Restart and persistence.** Leases are in memory only; a restart drops them (fail-closed), but no
  test restarts anything.
- **Performance under load.** One mutex, not benchmarked beyond the single-thread `revoke_all` time
  printed by `cargo run` (about 160-200 ns).
- Race tests exercise the window many times; they cannot prove it has no race. The linearizability
  argument rests on the single lock and code review, not a model checker.
- The two `expect`s guard the 2^64 lease-id and epoch counters. They are unreachable in practice and
  untested.
