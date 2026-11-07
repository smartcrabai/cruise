#![cfg(unix)]
use cruise::formal_verify::{Language, SpecBlock, Verdict, alloy};
use std::{fs, os::unix::fs::PermissionsExt, time::Duration};

#[test]
fn nonzero_alloy_exit_cannot_pass_with_a_valid_receipt() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let java = bin.join("java");
    fs::write(&java, r##"#!/bin/sh
if [ "$1" = '-version' ]; then echo 'openjdk version "17.0.1"' >&2; exit 0; fi
for arg in "$@"; do
  if [ "$arg" = commands ]; then echo '0 . Run witness for 3'; exit 0; fi
done
while [ "$#" -gt 0 ]; do
  if [ "$1" = '--output' ]; then shift; out="$1"; fi
  shift
done
mkdir -p "$out"
printf '%s' '{"commands":{"witness":{"type":"run","solution":[{"instances":[{}]}]}}}' > "$out/receipt.json"
exit 1
"##).unwrap();
    fs::set_permissions(&java, fs::Permissions::from_mode(0o755)).unwrap();
    let jar = dir.path().join("alloy.jar");
    fs::write(&jar, "fixture").unwrap();
    unsafe {
        std::env::set_var(
            "PATH",
            format!(
                "{}:{}",
                bin.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        );
        std::env::set_var("CRUISE_ALLOY_JAR", &jar);
    }
    let block = SpecBlock {
        language: Language::Alloy,
        number: 1,
        source: "run witness".into(),
    };
    let report = alloy::verify(&block, dir.path(), Duration::from_secs(5), None);
    assert_eq!(report.stages.len(), 1);
    assert_eq!(report.stages[0].verdict, Verdict::Error);
}
