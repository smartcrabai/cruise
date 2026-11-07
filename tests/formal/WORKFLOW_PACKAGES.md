# Workflow package formal models

These test-only models describe the design contract in the workflow package plan. They do not execute or prove the Rust implementation.

## Lifecycle (Quint)

`workflow_packages.qnt` models one initially installed old bundle. `stage`, `backup`, `promote`, `cleanupPrev`, `crash`, `recover`, `removeStart`, and `removeDone` represent the corresponding filesystem lifecycle. `before` and `event` are observers for transition properties. `lockMatches` abstracts the agreement of all visible files with the lock.

* `i1_whole`: a visible final directory is old or new, never a mixed bundle.
* `i2_lock`: a visible final directory matches its lock.
* `i3_recovery_cleanup`: recovery removes staging and removing entries.
* `i4_recovery_restore`: recovery restores the previous old bundle when final is absent.
* `i5_recovery_preserve`: recovery preserves an already visible final bundle.
* `i6_remove_no_resurrection`: recovery does not resurrect a removed bundle.
* The six executable tests cover successful replacement and crashes at staging, backup, promotion and removal.

The model assumes atomic directory renames, serialized operations, complete valid staging before promotion, and at most one previous directory. Stage represents even a partially written staging directory as invisible and discardable. It does not model actual bytes, hash computation, fsync durability, failed rollback or cleanup, multiple previous directories, concurrent readers, external modifications, or liveness. In particular, I1 is encoded by the final directory's abstract domain, not a proof of filesystem atomicity. Removal becomes externally absent at rename, including when deletion is interrupted. An update can temporarily have no visible final between backup and promotion. The model is not a proof that old or new is continuously available.

## Install collisions (Alloy)

`workflow_packages.als` models immutable entries for flat YAML, flat manifest, and bundle directories. An install requires that no entry of the requested name exists and adds either both flat entries or one bundle entry. `NoFlatBundleCollision` checks preservation of collision freedom from a valid starting state, and `PreservesExisting` checks existing entries remain in place. The two witness commands require satisfiable installs. It abstracts filenames into names and kinds, and does not model `.yml` separately, discovery, update, or a concurrent change between collision check and commit.

## Running the models

From the repository root, with the repository's pinned Quint and Alloy tools installed:

```sh
tests/formal/node_modules/.bin/quint typecheck tests/formal/workflow_packages.qnt
tests/formal/node_modules/.bin/quint test tests/formal/workflow_packages.qnt --backend typescript
tests/formal/node_modules/.bin/quint run --seed 20261107 --max-samples 1000 --max-steps 8 --invariant safety --backend typescript tests/formal/workflow_packages.qnt
node scripts/formal/verify_quint.cjs tests/formal/node_modules/.bin/quint tests/formal/workflow_packages.qnt workflow_packages 8 /tmp/workflow-packages-counterexample.itf.json
java -jar target/formal-tools/alloy-6.2.0.jar exec --command '*' --solver sat4j --type json --output /tmp/workflow-packages-alloy tests/formal/workflow_packages.als
node scripts/formal/check_alloy_receipt.cjs tests/formal/workflow_packages.als /tmp/workflow-packages-alloy/receipt.json
```

The Quint verification uses Apalache 0.62.1 and is bounded to 8 transitions. The simulation uses seed 20261107, 1000 samples and at most 8 steps. Alloy 6.2.0 uses the SAT4J solver and Temurin JDK 25.0.5+8. Alloy command scopes are specified in the model, with checks expecting no counterexamples and runs expecting instances. Both are bounded abstractions, not unbounded verification. The helper scripts named above already exist and are not changed by this work. Use fresh output paths for repeated runs. For the complete pinned setup, exact executed commands, logs and receipts, run `bash scripts/check_workflow_packages_specs.sh`; reports are written under `target/formal-reports/workflow-packages.*`.
