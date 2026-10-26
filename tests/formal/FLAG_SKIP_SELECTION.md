# Flag-dependent skip selection

## Authority and scope

These executable specifications cover the task in the session plan dated
2026-10-25: flag-disabled steps must not be skip candidates, flags are selected
before skips, and hidden selections must not survive into a newly created
session or its history. `NEVER.md` and the repository contribution requirements
remain applicable. The plan's R1-R3 and sections 1.1-1.3, 2.3-2.7 and 6 are the
requirements, not the current implementation's accidental behavior.

The existing `workflow_flags.qnt` and `.als` describe the execution-start
boundary and are deliberately unchanged. The new `flag_skip_selection.qnt` and
`.als` describe approval/new-session selection and Execute now forwarding. The
historical `WORKFLOW_FLAGS_VERIFICATION.md` is also unchanged.

| Required observable contract | Production entry points | Model / production evidence |
| --- | --- | --- |
| Absent or false flags hide gated candidates, ON restores them, a flag list uses OR | `flags::skip_candidates`, `is_disabled`, workflow node metadata | Quint `eligibility`, off/OR tests, Alloy candidate assertions and witnesses, Gherkin approval tree assertions, shared helper tests |
| Group parent and expanded children share the call-site gate, after-pr uses the same rule | `workflow::list_skippable_step_map`, CLI `skip_prompt_nodes`, TUI `skip_choices`, WebUI `step_tree_html` | Fixed representative group/after-pr in both models, actual YAML/CLI Gherkin, workflow/plan/TUI/WebUI unit tests |
| Choose flags before skips, explicit `-f` suppresses approval flag selection | `plan_cmd::run_approve_loop`, TUI `Step::ORDER`, `list_cmd` settings, WebUI flag rows | Quint `promptOrder`, Alloy ordering assertions, real PTY Gherkin and `flags_cli`, `flag_skip_list_cli`, TUI/WebUI tests |
| Cancelling flag/skip choice does not partially persist flags or skips or skip history | staged locals in `run_approve_loop`, `select_skipped_steps_with_history` | Quint `atomicity`, Alloy cancellation assertions/witnesses, real approval cancellation Gherkin and `flags_cli` |
| Prune disabled parent/expanded/after-pr IDs, preserve unknown free-text IDs | `flags::drop_disabled_skips`, TUI flag toggling, application creation | Quint `pruning`, API-create tests and Alloy pruning assertions/witnesses, application/TUI/helper tests |
| Newly saved session and selection history contain the sanitized selection | `application::create_session_inner`, approval skip history | Quint `atomicity`, Alloy history assertions/witnesses, production state/history assertions |
| All flags off can yield an empty list without enabling free-text fallback | TUI `all_skip_choices`, `step_is_text`, `render_skip_choices` | Quint full/all-gated/unknown catalogs and `allGatedOffStaysListTest`, TUI production tests |
| Execute now forwards declared keys, including explicit OFF, with no second flag prompt | `PlanDisposition::ExecuteNow`, `RunArgs.flags`, `should_prompt_flags` | Quint `forwarding`, Alloy forwarding assertions/witnesses, real PTY Gherkin and `flags_cli` |
| Resume options remain unfiltered, config changes retain their separate history/flags-off semantics | list `main_ids`, WebUI `settings_html`, `/webui/new/steps` versus `/webui/new/steps/flags` | Quint `eligibility`/resume test, Alloy resume assertion, real list PTY and WebUI route tests |

## Abstraction and limits

Both models use a finite representative workflow: an ungated step, a
single-flag step, an OR-gated step, a gated group parent with two expanded
children, an after-pr step, and an unknown free-text ID. Two declared flags and
an undeclared saved key demonstrate that retained legacy values cannot leak
into Execute now CLI assignments. Enabled-key sets abstract absent and explicit
false identically for eligibility. Forwarded-key sets distinguish explicit OFF
from no assignment. With inferred approval choices, forwarding includes every
declared key, including OFF. An existing explicit CLI assignment vector instead
retains exactly its supplied names and values, even when it assigns only one
of the two declared flags. Quint's `partialCliTest` and `cliAuthority` check this
case. Explicit CLI values have already been validated and persisted by
`apply_cli_flags_or_discard` before this approval boundary begins. The model's
initial saved state includes them. `explicitCancellationTest` requires cancellation
to retain that baseline, not undo earlier CLI assignment persistence. `cliAuthority`
requires approval to leave that whole supplied saved flag map unchanged. Sets
abstract ordering and duplicate skip IDs. Actual
workflow order, labels, sparse false persistence and group expansion are checked
against production Rust, not proved by the abstract sets.

The observable lifecycle stages are selection/approval, skip editing,
commit/cancellation and execution entry. A successful confirmation abstracts
successful approval finalization and save, not filesystem crash durability or
failure after a confirmed skip choice. Cancellation here is cancellation of a
selection prompt. No claim is made about a later title/workspace/save failure.
TUI/WebUI flag changes prune staged choices, while CLI stages flag changes in a
local value until skip confirmation. A model's staged skip set is the effective
selection, not an assertion about when the CLI rewrites its local session struct.
Unknown IDs are preserved by pruning, not necessarily by a list chooser that
only offers known IDs. History in the model is the skip-selection record, not
all fields in `history.json`.

The models abstract successful validated application creation. Invalid-name
rejection is not a Quint safety claim. Alloy additionally has an invalid request
witness demonstrating that other valid approval traces remain satisfiable while
that request cannot create a session. Alloy fixes a nonempty catalog and models
one approval attempt through six transitions. Quint also exercises empty catalogs,
explicit-CLI prompt suppression and returning to the menu after cancellation.
Its prompt counters describe the current approval attempt and forwarded execution
entry, not cumulative prompts across the session lifetime. Reopening approval
after cancellation may legitimately prompt again.

The Quint catalog varies between the full workflow, the gated-only workflow and
an unknown/empty catalog to distinguish filtered-empty lists from text fallback.
Resume options are the main expanded IDs only, including disabled steps, not
group parents or after-pr IDs. No graph execution algorithm, provider calls,
noninteractive auto-approval, exec/force_exec redesign, free-text settings editor
restriction, liveness, fairness, unbounded proof or Rust refinement proof is
claimed. The models state safety properties independently of their transition
updates. Safety is not assumed as a fact to constrain away counterexamples.

## Reproducible tools and commands

Use the existing install setup, no second dependency scheme:

```sh
# Prerequisites on PATH: Node 22.22.0, Rust 1.99.1, curl, tar, POSIX PTY.
bash scripts/check_workflow_flags_specs.sh
```

The runner invokes `scripts/setup_formal.sh`, requires supported macOS arm64 or
Linux x64, and fails if setup is missing or fails. The shared pins are Quint
**0.33.0** (`tests/formal/package.json` and integrity-pinned `package-lock.json`),
Apalache **0.62.1** (explicit backend version and isolated local endpoint),
Alloy CLI **6.2.0**, and Temurin **25.0.5+8-LTS**. The released Alloy JAR SHA-256
is `6b8c1cb5bc93bedfc7c61435c4e1ab6e688a242dc702a394628d9a9801edb78d`.
The JDK archive SHA-256 pins in `scripts/setup_formal.sh` are
`36dc14d797aaff6d89aab302cd053fdba6207dfdcf772052fa7b023a94b0e857`
(macOS arm64) and
`68982529869257d0ee6b824ec5aa270ff8b118bb438670da0b97ebdab178d7fb`
(Linux x64). Setup verifies cached archives too and checks the actual runtime.
Cucumber **0.23.0**, portable-pty **0.9.0** and tempfile **3.27.0** already use
exact Cargo pins and the committed root lockfile. All Rust commands use
`+1.99.1 --locked --all-features`.

The installed CLI help, versions and solver inventory are captured on every
aggregate invocation. In the commands below, `Q` is
`tests/formal/node_modules/.bin/quint`, `R` is the fresh absolute report directory
and `PORT` is allocated by `scripts/formal/verify_quint.cjs`:

```sh
"$Q" typecheck tests/formal/flag_skip_selection.qnt
"$Q" test tests/formal/flag_skip_selection.qnt --backend typescript --seed 20261026 --out-itf "$R/skip_test_{test}_{seq}.itf.json"
"$Q" run --seed 20261026 --max-samples 1000 --max-steps 8 --invariant safety --backend typescript --out-itf "$R/skip_simulation_{seq}.itf.json" tests/formal/flag_skip_selection.qnt
"$Q" verify --invariant safety --max-steps 8 --main flag_skip_selection --init init --step step --backend apalache --apalache-version 0.62.1 --server-endpoint "127.0.0.1:$PORT" --out-itf "$R/skip-counterexample.itf.json" tests/formal/flag_skip_selection.qnt
java -jar target/formal-tools/alloy-6.2.0.jar help exec
java -jar target/formal-tools/alloy-6.2.0.jar exec --command '*' --solver sat4j --type json --output "$R/skip-alloy" tests/formal/flag_skip_selection.als
node scripts/formal/check_alloy_receipt.cjs tests/formal/flag_skip_selection.als "$R/skip-alloy/receipt.json"
node --test scripts/formal/check_alloy_receipt.test.cjs
cargo +1.99.1 test --locked --all-features --test workflow_flags_bdd
node scripts/formal/check_workflow_flags_bdd_failures.cjs "$R"
cargo +1.99.1 test --locked --all-features --test flags_contract --test flags_cli --test flag_skip_list_cli
cargo +1.99.1 test --locked --all-features --lib --bin cruise-runtime
```

The helper executes the actual Quint `verify` command above, logs expanded argv,
and isolates backend output beneath `R`. Simulation is not proof: seed
**20261026**, **1000 samples**, **8 steps**. Separate Apalache symbolic
verification checks `safety` through **8 transitions**. See the dated execution
report for Alloy scopes, assertions, witnesses and actual receipts.

Alloy 6.2.0's installed `help exec` has no `--expect` option. Supported model
commands instead explicitly specify `expect 0` for checks and `expect 1` for
witnesses. `--command '*'` selects every command rather than relying on its
default. The shared receipt validator independently rejects unexpected SAT or
UNSAT, missing/extra/wrong-type results and malformed solutions, even when the
CLI itself exits zero. Its own negative unit tests run in CI.

## CI and failure semantics

`.github/workflows/ci.yml`'s `workflow-flags-specs` job runs the aggregate command
on relevant pull requests and pushes to main with read-only contents permission,
pinned Node/Rust, no secrets, and no external agent/model account. The runner
now executes both the unchanged execution-start models and the new skip-selection
models, strict production Gherkin, failure controls and production regression
tests. Missing prerequisites are failures, not skips. Independent checks still
run after a failure, and their accumulated exit status is nonzero.

The existing Rust-native Cucumber runner exercises the real launcher/runtime,
real session/application storage and real PTYs. `fail_on_skipped` rejects
undefined/pending/skipped steps. Assertions fail the native test process, and
zero executed scenarios are rejected. Negative controls require actual native
rejection of an undefined step, a deliberately false production assertion and
an empty filter, not a compiler failure, timeout or missing platform.

The Rust checks receive fresh report-local XDG config/data/state directories so
host cruise model defaults and history cannot affect results. The runner leaves
HOME and the inherited commit guard unchanged. Individual Rust fixtures use
fake homes under the existing process lock where application defaults are loaded.
If a local tool-manager shim itself
depends on XDG configuration, put the installed direct Rustup executables on
PATH before running, for example `PATH="$HOME/.cargo/bin:$PATH"`. This is not
needed on the clean GitHub runner.

Every run gets a fresh `target/formal-reports/workflow-flags.*/` directory.
Commands, status codes, actual help, runtime versions, Quint traces, verification
logs, Alloy receipt and concrete witness JSON are retained. GitHub uploads the
reports with `if: always()` for 14 days. Local results do not claim remote CI
execution or branch-protection enforcement. See `FLAG_SKIP_SELECTION_VERIFICATION.md`.
