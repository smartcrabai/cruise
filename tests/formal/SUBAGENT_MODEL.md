# Jcode subagent model specifications

The supported `subagent_model` configuration is a plain model string. A prompt-step value overrides the workflow value; otherwise the workflow value is used. Command workflows and malformed, blank, or effort-suffixed values are rejected before prompt dispatch. For Jcode, Cruise configures the `Agents ["swarm", route]` runtime setting before sending the prompt; a failed setting must not send it. Title and auxiliary metadata requests do not inherit prompt-only overrides.

## Contract map

| Contract | Concrete coverage |
| --- | --- |
| Step value takes precedence over workflow value | `subagent_model_bdd`, `subagent_model_jcode_cli` |
| Malformed, blank, effort-suffixed, and command-mode selections reject before dispatch | `subagent_model_bdd`, `sdk_model_config_contract` |
| Jcode resolves and configures the route before prompt submission | `subagent_model_jcode_cli` wire-frame contracts |
| A runtime-setting failure never sends the prompt; absent overrides leave existing settings untouched | `subagent_model_jcode_cli` |
| Workflow generation and unrelated metadata exclude task-scoped overrides | Jcode workflow/configuration regression tests in the library suite |

The Quint and Alloy models cover a single Jcode prompt request. They abstract route values and the underlying runtime protocol; the Rust wire contracts exercise the actual Jcode API frames.

## Run

From the repository root, with the pinned Node and Rust toolchains available:

```sh
bash scripts/check_subagent_model_specs.sh
```

The runner uses the repository's shared formal toolchain and retains a fresh report under `target/formal-reports/subagent-model.*`. It does not require provider credentials or external model calls.
