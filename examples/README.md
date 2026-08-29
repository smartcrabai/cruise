# Examples

Companion files for [`docs/github-actions.md`](../docs/github-actions.md). See that doc for the full command reference, setup steps, inputs/outputs, and security notes.

| File | Use it when... |
|---|---|
| [`cruise.yml`](cruise.yml) | You want the baseline setup: Anthropic (or OpenAI) via the dedicated `anthropic_api_key`/`openai_api_key` inputs, no custom model or config. Start here. |
| [`cruise-kimi.yml`](cruise-kimi.yml) | You want to drive cruise with [Kimi for Coding](https://api.kimi.com/coding/), a jcode built-in provider (id `kimi`) authenticated by `KIMI_API_KEY`. |
| [`cruise-openai-compatible.yml`](cruise-openai-compatible.yml) | Your models live behind an OpenAI-compatible endpoint not already known to jcode. Uses the `providers`/`provider_api_keys` inputs, which cover custom headers (`auth`/`auth_header`), OpenRouter-style routing (`provider_routing`), and keyless (`no_auth`) endpoints. |
| [`repo-cruise.yaml`](repo-cruise.yaml) | You want to commit your own cruise workflow config (default `jcode` backend, `write-tests -> implement -> test` with a fix-and-retry loop) instead of relying on the action's generated default. Copy it to your repository root as `cruise.yaml`. |

All three `cruise*.yml` files are complete, drop-in `.github/workflows/cruise.yml` replacements -- pick one, copy it, and fill in the secrets it references. `repo-cruise.yaml` is not a GitHub Actions workflow; it's a cruise config file that lives alongside your project's own source.
