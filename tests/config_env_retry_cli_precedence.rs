use std::process::Command;

#[test]
fn exec_retry_flag_takes_precedence_over_environment() {
    let temp = tempfile::tempdir().unwrap();
    let workflow = temp.path().join("workflow.yaml");
    std::fs::write(&workflow, "command: [echo]\nmax_retries: 1\ngroups:\n  review:\n    max_retries: 4\n    steps:\n      work:\n        command: echo work\nsteps:\n  main:\n    group: review\n").unwrap();
    let run = |env_retries: &str, flag: Option<&str>| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cruise"));
        for name in cruise::config_env::all_override_names() {
            command.env_remove(name);
        }
        command
            .current_dir(temp.path())
            .env("HOME", temp.path())
            .env("XDG_CONFIG_HOME", temp.path().join("config"))
            .env("CRUISE_MAX_RETRIES", env_retries)
            .args(["exec", "--dry-run", "-c"])
            .arg(&workflow);
        if let Some(value) = flag {
            command.args(["--max-retries", value]);
        }
        command.output().unwrap()
    };
    let env_wins = run("7", None);
    assert!(
        env_wins.status.success(),
        "{}",
        String::from_utf8_lossy(&env_wins.stderr)
    );
    let flag_low = run("7", Some("2"));
    assert!(
        !flag_low.status.success(),
        "flag must lower the effective retry budget"
    );
    assert!(String::from_utf8_lossy(&flag_low.stderr).contains("max_retries"));
    let flag_high = run("2", Some("7"));
    assert!(
        flag_high.status.success(),
        "{}",
        String::from_utf8_lossy(&flag_high.stderr)
    );
}
