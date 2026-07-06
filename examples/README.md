# Examples

Companion files for [`docs/github-actions.md`](../docs/github-actions.md). See that doc for the full command reference, setup steps, inputs/outputs, and security notes.

| File | Use it when... |
|---|---|
| [`cruise.yml`](cruise.yml) | You want the baseline setup: Anthropic (or OpenAI) via the dedicated `anthropic_api_key`/`openai_api_key` inputs, no custom model or config. Start here. |
| [`cruise-kimi.yml`](cruise-kimi.yml) | You want to drive cruise with [Kimi for Coding](https://api.kimi.com/coding/), a pi built-in provider authenticated by `KIMI_API_KEY`. Mirrors this repository's own dogfood workflow (`.github/workflows/cruise.yml`). |
| [`cruise-openai-compatible.yml`](cruise-openai-compatible.yml) | Your models live behind a self-hosted OpenAI-compatible endpoint (vLLM, LiteLLM, an internal gateway, ...) instead of a provider pi already knows. Uses `pi_models_json` to register it. |
| [`repo-cruise.yaml`](repo-cruise.yaml) | You want to commit your own cruise workflow config (`sdk: pi`, `write-tests -> implement -> test` with a fix-and-retry loop) instead of relying on the action's generated default. Copy it to your repository root as `cruise.yaml`. |

All three `cruise*.yml` files are complete, drop-in `.github/workflows/cruise.yml` replacements -- pick one, copy it, and fill in the secrets it references. `repo-cruise.yaml` is not a GitHub Actions workflow; it's a cruise config file that lives alongside your project's own source.
