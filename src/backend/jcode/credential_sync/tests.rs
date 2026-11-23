use super::*;

const LATER_MS: i64 = 1_900_000_000_000;
const EARLIER_MS: i64 = 1_800_000_000_000;

fn write_json(path: &Path, value: &Value) {
    fs::write(path, value.to_string()).unwrap_or_else(|error| panic!("{error}"));
}

fn now_ms() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_else(|error| panic!("{error}"))
            .as_millis(),
    )
    .unwrap_or_else(|error| panic!("{error}"))
}

#[test]
fn reclaim_persists_only_newer_rotations_of_existing_accounts() {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let source = temp.path().join("source");
    let private = temp.path().join("private");
    fs::create_dir_all(&source).unwrap_or_else(|error| panic!("{error}"));
    fs::create_dir_all(&private).unwrap_or_else(|error| panic!("{error}"));
    write_json(
        &source.join("openai-auth.json"),
        &serde_json::json!({
            "openai_accounts": [
                {"label": "rotated", "refresh_token": "consumed", "expires_at": EARLIER_MS},
                {"label": "newer-in-home", "refresh_token": "home", "expires_at": LATER_MS},
            ],
            "active_openai_account": "rotated",
        }),
    );
    write_json(
        &private.join("openai-auth.json"),
        &serde_json::json!({
            "openai_accounts": [
                {"label": "rotated", "refresh_token": "fresh", "expires_at": LATER_MS},
                {"label": "newer-in-home", "refresh_token": "stale", "expires_at": EARLIER_MS},
                {"label": "logged-out", "refresh_token": "gone", "expires_at": LATER_MS},
            ],
            "active_openai_account": "logged-out",
        }),
    );

    reclaim_rotated_credentials(&source, &private).unwrap_or_else(|error| panic!("{error}"));

    let shared = read_json(&source.join("openai-auth.json")).unwrap_or_else(|| panic!("store"));
    assert_eq!(
        shared,
        serde_json::json!({
            "openai_accounts": [
                {"label": "rotated", "refresh_token": "fresh", "expires_at": LATER_MS},
                {"label": "newer-in-home", "refresh_token": "home", "expires_at": LATER_MS},
            ],
            "active_openai_account": "rotated",
        })
    );
}

#[cfg(unix)]
#[test]
fn reclaim_ignores_stores_that_are_still_links() {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let source = temp.path().join("source");
    let private = temp.path().join("private");
    fs::create_dir_all(&source).unwrap_or_else(|error| panic!("{error}"));
    fs::create_dir_all(&private).unwrap_or_else(|error| panic!("{error}"));
    let other = temp.path().join("other.json");
    write_json(
        &other,
        &serde_json::json!({"anthropic_accounts": [{"label": "a", "expires": LATER_MS}]}),
    );
    let shared = serde_json::json!({"anthropic_accounts": [{"label": "a", "expires": EARLIER_MS}]});
    write_json(&source.join("auth.json"), &shared);
    std::os::unix::fs::symlink(&other, private.join("auth.json"))
        .unwrap_or_else(|error| panic!("{error}"));

    reclaim_rotated_credentials(&source, &private).unwrap_or_else(|error| panic!("{error}"));

    assert_eq!(read_json(&source.join("auth.json")), Some(shared));
}

#[cfg(unix)]
fn install_jcode_stub(dir: &Path, script: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("jcode");
    fs::write(&path, format!("#!/bin/sh\n{script}")).unwrap_or_else(|error| panic!("{error}"));
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
        .unwrap_or_else(|error| panic!("{error}"));
    path
}

#[cfg(unix)]
fn openai_store(expires_at: i64) -> Value {
    serde_json::json!({
        "openai_accounts": [{"label": "otter", "refresh_token": "rt", "expires_at": expires_at}],
        "active_openai_account": "otter",
    })
}

#[cfg(unix)]
#[test]
fn refresh_ahead_refreshes_an_expiring_login_once_in_the_user_home() {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let source = temp.path().join("source");
    fs::create_dir_all(&source).unwrap_or_else(|error| panic!("{error}"));
    write_json(
        &source.join("openai-auth.json"),
        &openai_store(now_ms() + 30 * 60 * 1000),
    );
    let calls = temp.path().join("calls");
    let fresh = now_ms() + 10 * 24 * 60 * 60 * 1000;
    let stub = install_jcode_stub(
        temp.path(),
        r#"echo "$JCODE_HOME $*" >> "$CALLS"
printf '{"openai_accounts":[{"label":"otter","refresh_token":"rotated","expires_at":%s}],"active_openai_account":"otter"}' "$FRESH" > "$JCODE_HOME/openai-auth.json"
"#,
    );
    let config = JcodeRunnerConfig {
        binary: Some(stub),
        provider: Some("openai".into()),
        cwd: Some(temp.path().to_path_buf()),
        env: std::collections::HashMap::from([
            ("CALLS".to_string(), calls.display().to_string()),
            ("FRESH".to_string(), fresh.to_string()),
            ("JCODE_HOME".to_string(), "/elsewhere".to_string()),
        ]),
        ..Default::default()
    };

    refresh_ahead(&config, &source).unwrap_or_else(|error| panic!("{error}"));
    refresh_ahead(&config, &source).unwrap_or_else(|error| panic!("{error}"));

    assert_eq!(
        fs::read_to_string(&calls).unwrap_or_else(|error| panic!("{error}")),
        format!(
            "{} auth-test --provider openai --no-smoke --no-tool-smoke\n",
            source.display()
        )
    );
    assert_eq!(
        OAuthStore::OpenAi.active_expiry_ms(&source.join("openai-auth.json")),
        Some(fresh)
    );
}

#[cfg(unix)]
#[test]
fn refresh_ahead_fails_only_when_the_unrefreshed_login_is_about_to_expire() {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let stub = install_jcode_stub(
        temp.path(),
        r#"cat <<'EOF'
=== auth-test: openai ===
✓ credential_probe — Loaded OpenAI OAuth credentials.
✗ refresh_probe — OpenAI token refresh failed: {
  "error": {
    "code": "refresh_token_reused"
  }
}
✓ provider_smoke — Skipped by --no-smoke.
result: FAIL
EOF
exit 1
"#,
    );
    let config = JcodeRunnerConfig {
        binary: Some(stub),
        provider: Some("openai".into()),
        cwd: Some(temp.path().to_path_buf()),
        ..Default::default()
    };
    let source = temp.path().join("source");
    fs::create_dir_all(&source).unwrap_or_else(|error| panic!("{error}"));

    write_json(
        &source.join("openai-auth.json"),
        &openai_store(now_ms() + 60 * 60 * 1000),
    );
    refresh_ahead(&config, &source)
        .unwrap_or_else(|error| panic!("a still-valid login must not block: {error}"));

    write_json(
        &source.join("openai-auth.json"),
        &openai_store(now_ms() - 1000),
    );
    let error = refresh_ahead(&config, &source)
        .err()
        .unwrap_or_else(|| panic!("an expired login must fail before launch"));
    assert_eq!(
        error,
        r#"jcode provider `openai` authentication expired and could not be refreshed: OpenAI token refresh failed: { "error": { "code": "refresh_token_reused" } }; run `jcode login openai`"#
    );
}
