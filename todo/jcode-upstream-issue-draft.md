# Upstream issue draft (1jehuang/jcode)

## Title

jcode-sdk private instances: inherited external credentials are rejected (symlink + path-bound trust) and model catalog caches are missing

## Body

### Summary

Instances started via `jcode_sdk` with a private `jcode_home` (`LaunchOptions.jcode_home` + `inherit_credentials`) behave differently from the user's normal jcode:

1. Inherited external credentials (e.g. `~/.config/github-copilot/hosts.json`) are unusable, so Copilot fails with `GitHub Copilot credentials not available. Run 'jcode login --provider copilot' first.`
2. Model catalog caches are not inherited, so `set_model` validates against the static list and rejects models the account can use, e.g. `Unsupported OpenAI model 'gpt-6-luna'`.

The same provider/model works with the plain CLI (`jcode run -p openai -m gpt-6-luna ...`).

### Environment

- jcode CLI v0.91.0 (macOS, arm64)
- jcode-sdk tag v0.90.0 (same code on `master` as of 0d002d211)

### Cause

**1. External credentials**

- `inherit_credentials` (`crates/jcode-sdk/src/launch.rs`) places every `EXTERNAL_CREDENTIAL_FILES` entry as a **symlink** at `<instance>/external/<relative>`
- With `JCODE_HOME` set, jcode reads them from `$JCODE_HOME/external/...` (`jcode_storage::user_home_path`), but `jcode_storage::validate_external_auth_file` refuses symlinks (`Refusing to read external auth file via symlink`)
- Even as a regular file, trust is path-bound: `Config::external_auth_source_allowed_for_path` requires `<source_id>|<canonical path>` in `[auth].trusted_external_source_paths`. The user's entry names the original path (`.../.config/github-copilot/hosts.json`), and the copied `config.toml` has no entry for the instance path

**2. Model catalog caches**

- `known_openai_model_ids()` (and the other providers' catalogs) read `*_model_catalog_cache.json` / `*_models_cache.json` from `jcode_storage::app_config_dir()`, which is `$JCODE_HOME/config/jcode` inside an instance
- `inherit_credentials` only links `*.env` from the user's app config dir, so the catalog caches are absent and `set_model` falls back to the static list before any live refresh

### Repro

```rust
for provider in ["openai", "copilot"] {
    let home = tempdir.path().join(provider);
    jcode_sdk::inherit_credentials(&jcode_sdk::user_jcode_home(), &home)?;
    let mut options = jcode_sdk::LaunchOptions {
        jcode_home: Some(home),
        inherit_logins: false,
        ..Default::default()
    };
    options.env.insert("JCODE_PROVIDER".into(), provider.into());
    let client = jcode_sdk::JcodeClient::launch(options)?;
    let session = client.create_session(None)?;
    println!("{:?}", client.set_model(&session.session_id, &format!("{provider}:gpt-6-luna")));
}
```

Output:

```
Err(Error { kind: Harness(InvalidRequest), message: "Unsupported OpenAI model 'gpt-6-luna'. Use /model to choose from the models available to your account. ..." })
Err(Error { kind: Harness(InvalidRequest), message: "GitHub Copilot credentials not available. Run `jcode login --provider copilot` first. ..." })
```

Preconditions: a model present in the user's OpenAI catalog cache but not in the static list; Copilot authenticated via a trusted `~/.config/github-copilot/hosts.json`.

### Suggested fix

- In `inherit_credentials`, copy external credentials as owner-only regular files (or make jcode accept the SDK-created links), and carry over the user's path-bound trust for the instance path
- Also copy model catalog caches (`*_model_catalog_cache.json`, `*_models_cache.json`) from the user's app config dir into `<instance>/config/jcode/`
