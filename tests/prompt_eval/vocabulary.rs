use cruise::{
    config::{StepConfig, WorkflowConfig},
    variable::referenced_names,
};
use serde::Deserialize;
use std::{fs, path::Path};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Vocabulary {
    runtime: Vec<String>,
    generate: Vec<String>,
    workflow_inputs: Vec<String>,
}

fn check_text(path: &Path, text: &str, vocabulary: &Vocabulary, generate: bool, inputs: &[String]) {
    for name in referenced_names(text) {
        let allowed = if generate {
            vocabulary.generate.contains(&name)
        } else {
            vocabulary.runtime.contains(&name)
                || name.starts_with("file:")
                || (vocabulary.workflow_inputs.contains(&name)
                    && name.strip_prefix("inputs.").is_some_and(|input| inputs.iter().any(|declared| declared == input)))
        };
        assert!(
            allowed,
            "{}: unknown prompt variable {{{name}}}",
            path.display()
        );
    }
}
fn check_step(path: &Path, step: &StepConfig, vocabulary: &Vocabulary, inputs: &[String]) {
    if let Some(prompt) = &step.prompt {
        check_text(path, prompt, vocabulary, false, inputs);
    }
    if let Some(with) = &step.with {
        for value in with.values() {
            check_text(path, value, vocabulary, false, inputs);
        }
    }
    if let Some(parallel) = &step.parallel {
        for child in parallel.values() {
            check_step(path, child, vocabulary, inputs);
        }
    }
}
fn walk(path: &Path, vocabulary: &Vocabulary) {
    for entry in fs::read_dir(path).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            walk(&path, vocabulary);
            continue;
        }
        if !matches!(
            path.extension().and_then(|s| s.to_str()),
            Some("yaml" | "yml")
        ) {
            continue;
        }
        let text = fs::read_to_string(&path).unwrap();
        let workflow =
            WorkflowConfig::from_yaml(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        for step in workflow.steps.values().chain(workflow.after_pr.values()) {
            check_step(&path, step, vocabulary, &workflow.inputs);
        }
        for group in workflow.groups.values() {
            for step in group.steps.values() {
                check_step(&path, step, vocabulary, &workflow.inputs);
            }
        }
    }
}
fn vocabulary() -> Vocabulary {
    let path = super::root().join("tests/prompt_eval/vocabulary.yaml");
    serde_yaml::from_str(&fs::read_to_string(path).expect("read vocabulary"))
        .expect("parse vocabulary")
}
#[test]
fn all_prompt_vocabulary_is_known() {
    let root = super::root();
    let vocabulary = vocabulary();
    for entry in fs::read_dir(root.join("prompts")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|s| s.to_str()) == Some("md") {
            let generate = path.file_name().is_some_and(|name| name == "workflow-generate.md");
            check_text(&path, &fs::read_to_string(&path).unwrap(), &vocabulary, generate, &[]);
        }
    }
    walk(&root.join("builtin"), &vocabulary);
}
#[test]
fn escaped_braces_are_not_references() {
    check_text(
        Path::new("test"),
        "{{headRefOid}} {input} {file:plan.md}",
        &vocabulary(),
        false,
        &[],
    );
}
#[test]
#[should_panic(expected = "unknown prompt variable {inputs.typo}")]
fn unknown_workflow_inputs_are_rejected() {
    check_text(Path::new("test"), "{inputs.typo}", &vocabulary(), false, &[]);
}

#[test]
fn generation_schema_is_accepted() {
    check_text(Path::new("workflow-generate.md"), "{schema}", &vocabulary(), true, &[]);
}

#[test]
#[should_panic(expected = "unknown prompt variable {schema}")]
fn runtime_schema_is_rejected() {
    check_text(Path::new("runtime-step"), "{schema}", &vocabulary(), false, &[]);
}
