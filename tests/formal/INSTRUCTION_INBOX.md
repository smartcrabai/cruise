# Instruction inbox protocol model

`instruction_inbox.qnt` is the existing Quint model of an instruction inbox. It abstracts the exclusive ledger lock as atomic actions. The finite model permits IDs 0 through 2, deduplication keys 0 and 1, and unkeyed requests represented by -1. It does not establish that the Rust implementation refines the model.

## State and transitions

The state records whether a run is open, accepted IDs and their pending, leased, delivered or expired dispositions, the next ID, and the two key-to-ID mappings. It also records the previous state and the latest event and request key for transition properties.

- `openRun` opens a closed run and returns any crash-stranded lease to pending.
- `accept` first recognizes an existing key as a duplicate, including when closed. Otherwise it rejects a closed run or, while open and below the ID bound, adds a new pending ID. Unkeyed requests are not deduplicated.
- `lease` takes the entire pending snapshot only when open and no lease is outstanding. Later accepted IDs remain pending. `ack` delivers the lease, and `release` returns it to pending.
- A completed close expires pending and leased work. Failed or suspended close preserves both as pending. `crash` closes the run without clearing the lease, and reopening releases it. `expire` clears outstanding work into expired while closed. `idle` leaves the state unchanged.

The whole-snapshot lease abstracts sequential and parallel-block delivery rather than modeling individual consumers or actual scheduling.

## Safety properties

The aggregate `safety` combines:

| Property | Model statement |
| --- | --- |
| `f1_partition` | Accepted IDs are exactly partitioned into pending, leased, delivered and expired, and correspond to allocated IDs. |
| `f2_one_lease` | A lease starts only with no existing lease and captures the pending snapshot. Acceptance during a lease does not change it. |
| `f3_no_closed_accept` | New acceptance requires an open run, and closed rejection leaves state unchanged. |
| `f4_terminal` | Delivered and expired sets are monotonic, and previously terminal IDs cannot return to pending. |
| `f5_dedupe` | Duplicate events preserve state for a previously mapped key, and new acceptance increments the ID only for an unmapped key. |
| `f6_close` | Completed close clears outstanding work, failed or suspended close carries it forward, and reopening returns a stranded lease to pending. |

The five executable scenarios cover completed close, carry-forward and acknowledgement, crash recovery, keyed deduplication, and explicit expiration. They are examples, not exhaustive verification.

## Scope and execution

The model does not represent filesystem failures, persistence details, transport, exact concurrent interleavings inside an atomic action, fairness, or liveness. Finite simulation is not proof, and bounded solver checking, if completed successfully, would not be an unbounded proof. Run `bash scripts/check_instruction_inbox_specs.sh` to produce a fresh report under `target/formal-reports/`. See `INSTRUCTION_INBOX_VERIFICATION.md` for the observed results and the pending solver status.
