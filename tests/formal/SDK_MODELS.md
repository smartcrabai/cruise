# Jcode model fallback specifications

These executable contracts cover ordered `model` and `plan_model` candidate strings for the only supported SDK, Jcode. Plain strings remain valid in workflow, app, retry-chain, environment-override, and session-snapshot configuration. Scheduling objects containing only `model` and `disabled_during` remain supported. SDK-qualified objects are rejected; provider references such as `claude/claude-opus-4-5` remain Jcode model references.

## Contract map

| Contract | Concrete coverage |
| --- | --- |
| Ordered plain-string execution and planning candidates survive compilation, defaults, app config and session snapshots | `sdk_model_bdd`, `sdk_model_execution_cli`, `model_fallback`, `app_config_cli`, `session_config_contract` |
| Explicit retry fallback strings take precedence and scheduled candidates preserve their windows | `model_fallback`, `model_schedule_schema` |
| SDK-qualified candidate/subagent objects are rejected with migration guidance | `sdk_model_config_contract`, `app_config_cli` |
| The only accepted SDK is Jcode, while Anthropic provider model references remain plain model strings | `sdk_model_config_contract`, `config_env_cli` |

The Quint and Alloy models abstract a Jcode candidate chain with a repeated effort variant. They assert ordered fallback, deduplication of repeated model identity, and the barrier against fallback after visible output. They do not model provider availability, backoff timing, or arbitrary chain lengths.

## Run

From the repository root, with the pinned Node and Rust toolchains available:

```sh
bash scripts/check_sdk_model_specs.sh
```

The runner uses the existing pinned Quint, Apalache, Alloy, and Cucumber tools, retains a fresh report directory under `target/formal-reports/sdk-models.*`, and runs the relevant Rust integration and library contracts. No provider credentials, external agent executable, or paid API call is required.
