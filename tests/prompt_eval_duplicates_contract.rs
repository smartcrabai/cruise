//! Known differences between independently shipped prompts must be reviewed when either changes.
use cruise::config::WorkflowConfig;
use sha2::{Digest, Sha256};
use std::{fs, path::Path};

fn hash(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        result.push(char::from(HEX[usize::from(byte >> 4)]));
        result.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    result
}

fn check_pair(
    name: &str,
    file_hash: &str,
    step_hash: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let file = fs::read(root.join(format!("prompts/{name}.md")))?;
    let yaml = fs::read_to_string(root.join("builtin/tdd.yaml"))?;
    let config = WorkflowConfig::from_yaml(&yaml)?;
    let prompt = config
        .steps
        .get(name)
        .ok_or_else(|| format!("missing step: {name}"))?
        .prompt
        .as_ref()
        .ok_or_else(|| format!("missing prompt for step: {name}"))?;
    assert_eq!(
        hash(&file),
        file_hash,
        "{name} file changed: review the known divergence"
    );
    assert_eq!(
        hash(prompt.as_bytes()),
        step_hash,
        "{name} workflow prompt changed: review the known divergence"
    );
    Ok(())
}

#[test]
fn write_test_first_known_difference_is_reviewed() -> Result<(), Box<dyn std::error::Error>> {
    check_pair(
        "write-test-first",
        "ff7e29d2890fe3c6b5358076d1f1b9ee607dbee6606be65c18a7e412a012c86d",
        "b6e4f7b1b1fb5a81176deca7a6a24fea866375322a84c23a8d31d582f31b66b5",
    )
}

#[test]
fn implement_after_tests_known_difference_is_reviewed() -> Result<(), Box<dyn std::error::Error>> {
    // Baseline updated after escaping literal example braces in the prompt file.
    check_pair(
        "implement-after-tests",
        "4f4a955923d886a7e96d131edc6ca4b41a41c4276c3339394dfdf2eb9ef8e0f6",
        "3d9a3e08a0ec2303398c8fc9cbeebd8c8baed1fc13cc972dd404e4c5fddf9e40",
    )
}
