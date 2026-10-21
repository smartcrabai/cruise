# Examples

Companion files for [`docs/github-actions.md`](../docs/github-actions.md). See that doc for the full command reference, setup steps, inputs/outputs, and security notes.

| File | Use it when... |
|---|---|
| [`cruise.yml`](cruise.yml) | You want the baseline setup: Anthropic (or OpenAI) credentials via the dedicated `anthropic_api_key`/`openai_api_key` inputs and a model from the `CRUISE_MODEL` repository variable. |
| [`cruise-kimi.yml`](cruise-kimi.yml) | You want to provide `KIMI_API_KEY` and a compatible Cruise model route to the action's jcode SDK runtime. |
| [`cruise-openai-compatible.yml`](cruise-openai-compatible.yml) | You want the action to provision a jcode profile for an OpenAI-compatible endpoint with `providers`/`provider_api_keys`, and explicitly pass the matching `provider/model` route to Cruise. |
| [`repo-cruise.yaml`](repo-cruise.yaml) | You want to commit your own cruise workflow config (`jcode` fallback only when neither `command` nor `sdk` remains set in the resolved workflow after applicable `default.yaml` layers, `write-tests -> implement -> test` with a fix-and-retry loop) instead of relying on the action's generated default. Copy it to your repository root as `cruise.yaml`. |
| [`file-artifacts.yaml`](file-artifacts.yaml) | You want to save a prompt's initial-state response in the session and read it later for a regression comparison. |

The `cruise*.yml` files are GitHub Actions workflow templates; copy a suitable one, fill in the secrets it references, and configure a model route available to the selected credentials. A jcode provider profile's own default model does not replace Cruise's `model` setting. `repo-cruise.yaml` and `file-artifacts.yaml` are cruise config files that live alongside your project's own source, not Actions workflows.
