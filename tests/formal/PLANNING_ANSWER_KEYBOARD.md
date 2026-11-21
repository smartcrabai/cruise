# Planning answer keyboard contract

Run the same fail-closed entry point as `.github/workflows/responsive-webui-specs.yml` from the repository root:

```sh
bash scripts/check_responsive_webui_specs.sh
```

No provider credentials are required. The fixture uses the production daemon and a local command workflow, authenticates using the daemon's one-time browser login, and creates disposable Git repositories outside the project. CI uploads `target/formal-reports/` even on failure. `commands.log` and `results.log` record the exact commands and exit codes for each run.

## Contract and coverage

| Behavior | Quint (`ask_keyboard.qnt`) | Alloy (`planning_answer_keyboard.als`) | Production Cucumber/Chromium |
| --- | --- | --- | --- |
| Cmd or Ctrl with Enter on the Ask textarea | Both modifiers and owning session checked | Both modifiers have satisfiable submission witnesses | Both key combinations send the real POST |
| Native `required` and normal Enter | Invalid and plain Enter effects checked separately | Empty drafts do not post, plain Enter updates the draft | Native `invalid` event, newline present, no POST |
| Ignore unrelated targets and keys | Editor and unrelated targets cannot emit an Ask POST | Other-target Enter cannot post | Real Tell editor does not submit, modified arrow does not submit |
| Reload, request identity, and duplicate drop | Two owners, identity across reload, per-owner in-flight guard, and no replay on failure or acceptance | Reload and accept lifecycle, dropped repeat and retry after failure | Same request ID after reload, controls disabled during both attempts, intercepted HTTP 500 with no shortcut replay, and unchanged explicit retry accepted by the daemon |

The shortcut is scoped to the Planning answer textarea: the similarly shaped Editor form keeps its existing behavior. The models abstract browser/server implementation details and do not claim deduplication across tabs or during navigation. Chromium checks real DOM, SSE and HTTP with the vendored HTMax. For both modifiers, Chromium holds the first answer POST while dispatching a competing shortcut, then returns HTTP 500 without forwarding it. Both controls re-enable, the draft and request ID stay intact, and no POST replays before an explicit retry. Chromium holds that retry through another competing shortcut, then forwards it unchanged to the daemon. The panel clears with no duplicate POST. In the separate Send recovery scenario, Chromium intercepts the first POST with HTTP 500, checks that both controls re-enable, and forwards the retry with the original request ID and answer.

## Reproduce individual checks

```sh
bash scripts/setup_formal.sh
source target/formal-tools/env.sh
Q=tests/formal/node_modules/.bin/quint
mkdir -p target/formal-reports/ask-keyboard
R=$(mktemp -d "$PWD/target/formal-reports/ask-keyboard/run.XXXXXX")
"$Q" typecheck tests/formal/ask_keyboard.qnt
"$Q" test tests/formal/ask_keyboard.qnt --backend typescript --seed 20261120
"$Q" run --seed 20261120 --max-samples 1000 --max-steps 20 --backend typescript --invariant safety --out-itf "$R/simulation_{seq}.itf.json" tests/formal/ask_keyboard.qnt
node scripts/formal/verify_quint.cjs "$Q" tests/formal/ask_keyboard.qnt ask_keyboard 8 "$R/counterexample.itf.json"
java -jar target/formal-tools/alloy-6.2.0.jar help exec
java -jar target/formal-tools/alloy-6.2.0.jar exec --command '*' --solver sat4j --type json --output "$R/alloy" tests/formal/planning_answer_keyboard.als
node scripts/formal/check_alloy_receipt.cjs tests/formal/planning_answer_keyboard.als "$R/alloy/receipt.json"
bash scripts/check_responsive_webui_bdd.sh
```

- **Quint 0.33.0**, locked by `tests/formal/package-lock.json`. The verification helper invokes actual `quint verify --invariant safety --max-steps 8 --main ask_keyboard --init init --step step --backend apalache --apalache-version 0.62.1` with an isolated endpoint and logs the full argv. The random simulation is not proof. Apalache verification is bounded to eight transitions in the model.
- **Alloy 6.2.0 / SAT4J**. The release JAR SHA-256 is `6b8c1cb5bc93bedfc7c61435c4e1ab6e688a242dc702a394628d9a9801edb78d`. The pinned CLI has no `--expect` flag: model commands declare `expect 0` for checks and `expect 1` for runs. The receipt checker fails on missing/extra commands, counterexamples or missing witnesses. The assertion scope is eight states and two request atoms; witness scopes are seven or eight states and exactly two request atoms.
- **Temurin 25.0.5+8-LTS** and **Apalache 0.62.1** are checksum-pinned by `scripts/setup_formal.sh`; the supported local formal platforms are macOS arm64 and Linux x64. **Node 26.13.1**, **Rust 1.99.1**, **Cucumber 0.23.0** and **Playwright 1.62.1** are the shared CI pins. The browser setup installs Playwright's pinned Chromium, and Cucumber fails on skipped (including undefined) steps and assertion failures.

## Local execution evidence

On macOS arm64, the Planning model typechecked, all five Quint tests passed, and the seeded simulation found no violation in 1,000 samples with up to 20 transitions per sample. Apalache reported `NoError` through state 8. The pinned Alloy CLI help was inspected before execution. The receipt contained exactly these results:

| Expected result | Receipt commands |
| --- | --- |
| UNSAT, no counterexample (`expect 0`) | `PlainEnterEditsWithoutPosting`, `RestrictedSubmissions`, `ReloadRetainsQuestionAndAcceptanceClearsIt` |
| SAT, witness found (`expect 1`) | `CmdLifecycle`, `CtrlValidatesAndDropsRepeat`, `OtherTargetAndButtonRecovery`, `OnlyNewRequestAfterAcceptance` |

The production changes address two defects: the promised shortcut had no delegated listener, and the Ask form did not drop overlapping requests. A shared Cargo cache also exposed stale worktree paths in the Cucumber executable. The runner now resolves fixtures from Cargo's runtime package directory and rebases in-worktree binary paths. The browser scenarios passed for both modifiers in Chromium 151.0.7922.34. The same Cucumber binary ran all nine responsive regressions, including the real Option dialog. `node tests/webui/option-dialog.mjs` also passed separately. No local prerequisite was missing. The Actions job is configured but was not run from this worktree.

For the local aggregate, see `target/formal-reports/responsive-webui.9TJLUk/commands.log`, `results.log`, `planning-answer-quint-verify.log`, and `planning-answer-alloy/receipt.json`. A rerun creates a fresh report directory. `bash scripts/check_responsive_webui_bdd.sh` records the browser and runner output under `target/formal-reports/responsive-browser.*`.
