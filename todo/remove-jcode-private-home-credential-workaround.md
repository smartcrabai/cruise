# Remove the cruise-side jcode private-home credential/cache workaround

Run this prompt once upstream jcode (`1jehuang/jcode`) ships a fix and cruise's
`jcode-sdk` dependency is bumped to a tag that contains it.

## Prompt

```
Repo: cruise. Upstream jcode has fixed private SDK instance homes so that cruise's
workaround in src/backend/jcode.rs is no longer needed. Verify, then remove it.

# Background
cruise runs each jcode SDK turn in a private JCODE_HOME built by
`prepare_session_home` (src/backend/jcode.rs), which calls
`jcode_sdk::inherit_credentials`. Up to jcode-sdk v0.90.0 / jcode v0.91.0 that
produced two failures, worked around in cruise:
1. Model catalog caches in the user's app config dir
   (`~/Library/Application Support/jcode/*_model_catalog_cache.json`,
   `*_models_cache.json`) were not inherited, so `set_model` validated against
   jcode's static model list -> e.g. `Unsupported OpenAI model 'gpt-6-luna'`.
   Workaround: `copy_private_model_catalog_caches` + `MODEL_CATALOG_CACHE_FILES`.
2. External credentials (`EXTERNAL_CREDENTIAL_FILES`, e.g.
   `~/.config/github-copilot/hosts.json`) were placed as symlinks under
   `<instance>/external/`, but `jcode_storage::validate_external_auth_file`
   refuses symlinks and `[auth].trusted_external_source_paths` trust is bound to
   the canonical original path -> `GitHub Copilot credentials not available`.
   Workaround: `materialize_external_credentials`,
   `materialize_external_credentials_in`, `materialize_external_credential`,
   `is_openclaw_agent_credential`, `VALIDATED_EXTERNAL_CREDENTIAL_FILES`,
   `MaterializedExternalCredential`, `copy_external_auth_trust`,
   `atomically_copy_private_file`, `PRIVATE_COPY_SEQUENCE`.

# Steps
1. Confirm the upstream fix in the jcode-sdk tag pinned in Cargo.toml
   (read the checkout under ~/.cargo/git/checkouts/jcode-*/<rev>/crates/jcode-sdk/src/launch.rs
   and jcode-base/jcode-storage): `inherit_credentials` must (a) make catalog caches
   available in the instance app config dir and (b) make external credentials
   readable by jcode (no symlink rejection) with the user's path-bound trust
   honored. Also confirm the installed jcode CLI version contains the
   jcode-side half if the fix spans both. If either part is missing, stop and
   report exactly what is still missing; remove only the parts that are fixed.
2. Remove the obsolete workaround code and its call sites in
   `prepare_session_home`, plus any helpers/constants/imports only they use.
   Keep unrelated private-home logic (aliases, socket preflight, locks, prune).
3. Remove the tests that only cover the workaround
   (`private_home_copies_catalogs_and_external_auth_idempotently` and its
   `assert_private_*` helpers in src/backend/jcode/tests.rs).
4. README.md (SDK mode private-home paragraph): drop the sentence about cruise
   copying model catalog caches and external credentials.
5. Verify: cargo fmt; cargo clippy --all-targets --all-features -- -D warnings
   -A clippy::large_futures; cargo test --no-run, then run the cruise unit test
   binary filtered to `backend::jcode`.
6. Smoke (macOS, real jcode): via a throwaway ignored test calling
   `stream_agent` with provider/model pairs `openai/<newest model in the user's
   catalog>` and `copilot/<model>` (cwd = temp dir, prompt "reply ok"), confirm
   both return text instead of `Unsupported OpenAI model` /
   `GitHub Copilot credentials not available`. Delete the throwaway test.
```

## Upstream tracking

- Issue: https://github.com/1jehuang/jcode/issues/1747 (filed 2026-10-07)
