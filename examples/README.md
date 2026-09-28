# Examples

Companion files for [`docs/github-actions.md`](../docs/github-actions.md). See that doc for the full command reference, setup steps, inputs/outputs, and security notes.

| File | Use it when... |
|---|---|
| [`cruise.yml`](cruise.yml) | You want the baseline setup: Anthropic (or OpenAI) via the dedicated `anthropic_api_key`/`openai_api_key` inputs, no custom model or config. Start here. |
| [`cruise-kimi.yml`](cruise-kimi.yml) | You want to provide `KIMI_API_KEY` to the action's jcode SDK runtime; the example leaves model selection to the SDK default. |
| [`cruise-openai-compatible.yml`](cruise-openai-compatible.yml) | You want the action to provision a jcode profile for an OpenAI-compatible endpoint with `providers`/`provider_api_keys`. Model route selection is separate; see the SDK documentation. |
| [`repo-cruise.yaml`](repo-cruise.yaml) | You want to commit your own cruise workflow config (default `jcode` backend, `write-tests -> implement -> test` with a fix-and-retry loop) instead of relying on the action's generated default. Copy it to your repository root as `cruise.yaml`. |
| [`file-artifacts.yaml`](file-artifacts.yaml) | You want to save a prompt's initial-state response in the session and read it later for a regression comparison. |

The `cruise*.yml` files are GitHub Actions workflow templates; copy a suitable one and fill in the secrets it references. Review model selection against the jcode SDK configuration available to your run. `repo-cruise.yaml` and `file-artifacts.yaml` are cruise config files that live alongside your project's own source, not Actions workflows.
