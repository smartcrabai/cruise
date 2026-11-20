# Daemon executable specifications

Run `mise exec node@26.13.1 -- bash scripts/check_daemon_specs.sh` from the
repository root. The secret-free `.github/workflows/daemon-specs.yml` runs the
same check on pull requests and pushes to `main`, and uploads
`target/formal-reports/` even when a check fails. Each invocation records exact
commands and results in a fresh report directory.

The shared `scripts/setup_formal.sh` installs Node-locked Quint 0.33.0 via
`npm ci`, checksum-verifies Alloy 6.2.0 (SHA-256
`6b8c1cb5bc93bedfc7c61435c4e1ab6e688a242dc702a394628d9a9801edb78d`)
and Temurin 25.0.5+8, and selects Apalache 0.62.1. Node 26.13.1 and Rust
1.99.1 are required. Cucumber 0.23.0 is pinned in Cargo.lock. Alloy 6.2.0's
`help exec` specifies `--command`; expectations are model `expect 0` and
`expect 1` clauses, not an unsupported CLI `--expect` flag.

The runner typechecks, tests and simulates `tests/formal/daemon.qnt`, then
checks its `safety` invariant with Apalache for eight transitions. The named
Quint tests are explicitly required by the script because Quint otherwise
succeeds with zero discovered tests:

- Existing lifecycle guarantees: `daemonRoundTripTest`, `bindFailureTest`,
  `stalePidReuseTest`, `staleReplacementTest`, `signalMatchingIdentityTest`,
  `restartRoundTripTest`, and `parentHoldsLockTest`.
- New daemon contract: `autostartSingleOwnerTest`,
  `registryDisappearanceShutdownTest`, `registryTakeoverShutdownTest`,
  `unspecifiedPortFallbackTest`, `explicitPortStrictnessTest`,
  `versionMismatchIdleRestartTest`, `versionMismatchBusyRefusalTest`,
  `apiAuthorizationTest`, `loginNonceSingleUseTest`, and
  `webSessionSeparateFromApiTokenTest`.

The model represents CLI autostart as a client acquiring the same registry lock
as explicit start. A contending autostart waits while the parent holds the lock
through child publication/readiness; after publication it reuses the sole live
instance. Registry disappearance and takeover remove the old owner. The Gherkin
tests disappearance with a real daemon; takeover is model-only. The two bind
outcomes abstract an unspecified default-port collision falling back to
ephemeral binding and an explicit occupied port failing. A version mismatch
atomically replaces an idle owner, while a busy owner is refused. An API
request is accepted only when its supplied token matches the owner token. The
WebUI auth abstracts an API-token-authenticated issuance and single-use
consumption of one representative login nonce, plus a browser session ID
distinct from the API token. Nonce multiplicity and five-minute expiry are not
modeled. Sessions remain unchanged by lifecycle operations; stale PID reuse,
identity-checked signals, and status at the observation point remain modeled.

The model is bounded, not a refinement proof. It does not model actual locks,
network sockets, the bearer-token parser or constant-time comparison, HTTP
timeouts, filesystem durability, Windows process APIs, or liveness. Bind and
version outcomes are abstract transitions. In particular, version-mismatch
busy refusal is model-only: production tests do not synthesize active work just
to force that branch. `daemon_control` tests the idle mismatch path with a
mocked stale status response and a real daemon restart. The BDD separately
changes only registry version metadata and verifies that status reports the
running process rather than stale registry contents.

Legacy WebUI migration is a production lifecycle detail: a verified live record
blocks daemon startup, `daemon stop` can stop both servers, stale records are
cleaned only after identity checks, and `daemon status` reports a live legacy
process. This migration path is not modeled.

`tests/formal/daemon.als` checks eighteen assertions and six witnesses in
exactly seven ordered states (six transitions), with SAT4J. Its lock, token,
nonce, and browser-session fields are abstractions, not operating-system or
HTTP implementations. Alloy receipt validation compares every declared
expectation with the tool's receipt; the runner does not rely on Alloy's exit
code alone.

The production-connected Gherkin in `tests/features/daemon.feature`, implemented
by `tests/daemon_bdd.rs`, drives the shipped CLI and real listener. It covers
`cruise daemon start|stop|restart|status|serve`, autostart from
`cruise list --json`, registry-removal shutdown, default-port fallback,
explicit-port failure, stale registry-version handling, and bearer-token checks
for `GET /api/v1/status` and `POST /api/v1/shutdown`. The port fallback scenario
holds `127.0.0.1:8484` when available (or uses an existing occupant); the strict
port scenario holds a dynamically allocated port. Integration tests in
`tests/daemon_lifecycle.rs` and `tests/daemon_control.rs` retain the named
process-identity, stale-record, lock, shutdown, API-authentication, proxy, and
client-triggered version-restart checks.

The earlier dated run used the pre-rename model and does not verify this
expanded suite. Current verification evidence is recorded under
`target/formal-reports/` by the runner.
