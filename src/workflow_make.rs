//! Interactive workflow authoring. Drafts remain in the data directory until approved.
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

use crate::cli::WorkflowMakeArgs;
use crate::config::{PermissionMode, WorkflowConfig};
use crate::error::{CruiseError, Result};
use crate::workflow_generate::{
    RepairError, destination_path, run_prompt, run_with_repairs, validate_generated_yaml,
    write_new_workflow,
};

const TEMPLATE: &str = include_str!("../prompts/workflow-make.md");
const SCHEMA: &str = include_str!("../cruise-schema.json");
const TRANSCRIPT_BYTE_BUDGET: usize = 32 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ParsedReply {
    pub message: String,
    pub workflow: Option<String>,
}

pub(crate) fn parse_reply(reply: &str) -> std::result::Result<ParsedReply, String> {
    let lines: Vec<&str> = reply.lines().collect();
    let opens: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter_map(|(i, l)| (l.trim() == "<cruise-workflow>").then_some(i))
        .collect();
    let closes: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter_map(|(i, l)| (l.trim() == "</cruise-workflow>").then_some(i))
        .collect();
    if opens.is_empty() && closes.is_empty() {
        if reply.trim().is_empty() {
            return Err("reply is empty".into());
        }
        if lines.iter().any(|line| line.starts_with("steps:")) {
            return Err("workflow YAML must be enclosed in <cruise-workflow> tags".into());
        }
        return Ok(ParsedReply {
            message: reply.trim().to_owned(),
            workflow: None,
        });
    }
    if opens.len() != 1 || closes.len() != 1 || opens[0] >= closes[0] {
        return Err(
            "reply must contain exactly one <cruise-workflow> ... </cruise-workflow> block".into(),
        );
    }
    let mut content = &lines[opens[0] + 1..closes[0]];
    while content.first().is_some_and(|s| s.trim().is_empty()) {
        content = &content[1..];
    }
    while content.last().is_some_and(|s| s.trim().is_empty()) {
        content = &content[..content.len() - 1];
    }
    if content.len() >= 2
        && content[0].trim().starts_with("```")
        && content[content.len() - 1].trim() == "```"
    {
        content = &content[1..content.len() - 1];
        while content.first().is_some_and(|s| s.trim().is_empty()) {
            content = &content[1..];
        }
        while content.last().is_some_and(|s| s.trim().is_empty()) {
            content = &content[..content.len() - 1];
        }
    }
    if content.is_empty() {
        return Err("workflow block is empty".into());
    }
    let message = [
        lines[..opens[0]].join("\n"),
        lines[closes[0] + 1..].join("\n"),
    ]
    .into_iter()
    .map(|s| s.trim().to_owned())
    .filter(|s| !s.is_empty())
    .collect::<Vec<_>>()
    .join("\n");
    Ok(ParsedReply {
        message,
        workflow: Some(content.join("\n")),
    })
}

#[derive(Clone)]
pub(crate) enum Role {
    User,
    Assistant,
}
#[derive(Clone)]
pub(crate) struct Turn {
    pub role: Role,
    pub text: String,
}
pub(crate) fn render_transcript(turns: &[Turn], budget: usize) -> String {
    if turns.is_empty() {
        return "(no earlier turns)".into();
    }
    let mut chosen = Vec::new();
    let mut used = 0;
    for turn in turns.iter().rev() {
        let role = match turn.role {
            Role::User => "User",
            Role::Assistant => "Assistant",
        };
        let len = role.len() + ": ".len() + turn.text.len() + "\n".len();
        if len > budget - used {
            continue;
        }
        used += len;
        chosen.push(format!("{role}: {}\n", turn.text));
    }
    let omitted = turns.len() - chosen.len();
    chosen.reverse();
    let mut result = if omitted > 0 {
        format!("(earlier turns omitted: {omitted})\n")
    } else {
        String::new()
    };
    for entry in chosen {
        result.push_str(&entry);
    }
    result
}

pub(crate) fn build_turn_prompt(
    template: &str,
    schema: &str,
    reference: Option<(&str, &str)>,
    transcript: &str,
    draft: Option<&str>,
    request: &str,
    repair: Option<(&str, &str)>,
) -> String {
    // Scan only the template, never substituted content.
    let mut out = String::new();
    let mut rest = template;
    let reference = reference.map_or_else(
        || "(none)".to_owned(),
        |(label, text)| format!("{label}\n{text}"),
    );
    let repair = repair.map_or_else(String::new, |(previous, diagnostic)| format!("## Previous attempt\n\nYour previous reply was rejected. Fix it and return a complete workflow block.\n\nPrevious reply:\n\n{previous}\n\nDiagnostic:\n\n{diagnostic}\n"));
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let substitutions = [
            ("{schema}", schema),
            ("{reference}", reference.as_str()),
            ("{transcript}", transcript),
            ("{draft}", draft.unwrap_or("(no draft yet)")),
            ("{request}", request),
            ("{repair}", repair.as_str()),
        ];
        if let Some((key, value)) = substitutions.iter().find(|(key, _)| rest.starts_with(key)) {
            out.push_str(value);
            rest = &rest[key.len()..];
        } else {
            out.push('{');
            rest = &rest[1..];
        }
    }
    out.push_str(rest);
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Approval {
    Write,
    Continue,
    Cancel,
}
pub(crate) trait MakeInput {
    fn read_request(&mut self, prompt: &str) -> Result<Option<String>>;
    fn choose_approval(&mut self, dest: &Path) -> Result<Approval>;
}
struct TerminalMakeInput;
impl MakeInput for TerminalMakeInput {
    fn read_request(&mut self, prompt: &str) -> Result<Option<String>> {
        Ok(match crate::multiline_input::prompt_multiline(prompt)? {
            crate::multiline_input::InputResult::Submitted(s) => Some(s),
            crate::multiline_input::InputResult::Cancelled => None,
        })
    }
    fn choose_approval(&mut self, dest: &Path) -> Result<Approval> {
        crate::platform::reclaim_terminal_foreground();
        match inquire::Select::new(
            &format!("Write workflow to {}?", dest.display()),
            vec!["Write workflow", "Continue editing", "Cancel"],
        )
        .prompt()
        {
            Ok("Write workflow") => Ok(Approval::Write),
            Ok("Cancel") => Ok(Approval::Cancel),
            Ok(_)
            | Err(
                inquire::error::InquireError::OperationCanceled
                | inquire::error::InquireError::OperationInterrupted,
            ) => Ok(Approval::Continue),
            Err(e) => Err(CruiseError::Other(e.to_string())),
        }
    }
}

struct Conversation {
    config: WorkflowConfig,
    cwd: PathBuf,
    dest: PathBuf,
    staging: PathBuf,
    draft: PathBuf,
    reference: Option<String>,
    turns: Vec<Turn>,
}
impl Conversation {
    fn log(&self, event: &str, detail: &str) -> Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.staging.join("conversation.jsonl"))?;
        let value = serde_json::json!({"at": crate::session::current_timestamp_id(), "event": event, "detail": detail});
        writeln!(file, "{value}")?;
        Ok(())
    }
    async fn submit(&mut self, request: &str) -> Result<()> {
        self.log("request", request)?;
        let draft = match std::fs::read_to_string(&self.draft) {
            Ok(s) => Some(s),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        };
        let transcript = render_transcript(&self.turns, TRANSCRIPT_BYTE_BUDGET);
        let base = build_turn_prompt(
            TEMPLATE,
            SCHEMA,
            self.reference.as_deref().map(|s| ("Reference", s)),
            &transcript,
            draft.as_deref(),
            request,
            None,
        );
        let token = crate::cancellation::CancellationToken::new();
        let config = &self.config;
        let cwd = &self.cwd;
        let token_ref = &token;
        let parsed = run_with_repairs(
            base,
            |prompt| async move {
                let spinner = (std::io::stderr().is_terminal() && !crate::console_mode::is_quiet())
                    .then(|| crate::spinner::Spinner::start("Drafting workflow..."));
                let outcome = {
                    let operation = run_prompt(
                        config,
                        &prompt,
                        PermissionMode::ReadOnly,
                        Some(cwd),
                        Some(token_ref),
                    );
                    tokio::pin!(operation);
                    tokio::select! {
                        result = &mut operation => result,
                        _ = tokio::signal::ctrl_c() => {
                            token_ref.cancel();
                            let _ = operation.await;
                            Err(CruiseError::Interrupted)
                        }
                    }
                };
                drop(spinner);
                outcome
            },
            |reply| {
                parse_reply(reply)
                    .and_then(|p| {
                        if let Some(yaml) = &p.workflow {
                            validate_generated_yaml(yaml, self.dest.parent().unwrap_or(&self.cwd))
                                .map_err(|e| e.to_string())?;
                        }
                        Ok(p)
                    })
                    .map_err(CruiseError::Other)
            },
            |reply, err| {
                build_turn_prompt(
                    TEMPLATE,
                    SCHEMA,
                    self.reference.as_deref().map(|s| ("Reference", s)),
                    &transcript,
                    draft.as_deref(),
                    request,
                    Some((reply, &err.to_string())),
                )
            },
        )
        .await;
        self.finish_submit(request, parsed)
    }
    fn finish_submit(
        &mut self,
        request: &str,
        parsed: std::result::Result<ParsedReply, RepairError>,
    ) -> Result<()> {
        match parsed {
            Ok(p) => {
                if let Some(yaml) = &p.workflow {
                    crate::planning::write_plan_atomically(&self.draft, yaml.as_bytes())?;
                    if let Err(e) = self.log("draft_updated", "validated draft") {
                        eprintln!(
                            "Warning: could not append draft update to conversation log: {e}"
                        );
                    }
                    println!("Draft updated: {}", self.draft.display());
                } else {
                    self.log("message", &p.message)?;
                }
                if !p.message.is_empty() {
                    println!(
                        "{}",
                        crate::batch_dashboard::sanitize_terminal_text(&p.message)
                    );
                }
                self.turns.push(Turn {
                    role: Role::User,
                    text: request.to_owned(),
                });
                self.turns.push(Turn {
                    role: Role::Assistant,
                    text: p.message,
                });
                Ok(())
            }
            Err(RepairError::Backend(e)) => {
                self.log("backend_error", &e.to_string())?;
                Err(e)
            }
            Err(RepairError::Rejected(e)) => {
                self.log("rejected", &e.to_string())?;
                Err(e)
            }
        }
    }
    fn accept(&self) -> Result<()> {
        let yaml = std::fs::read_to_string(&self.draft)?;
        validate_generated_yaml(&yaml, self.dest.parent().unwrap_or(&self.cwd))?;
        write_new_workflow(&self.dest, &yaml)?;
        if let Err(e) = self.log("accepted", &self.dest.display().to_string()) {
            eprintln!("Warning: could not append acceptance to conversation log: {e}");
        }
        println!("Created {}", self.dest.display());
        println!(
            "Review the YAML before running it. Validation does not prove the commands are safe or that the workflow succeeds."
        );
        Ok(())
    }
    async fn interact(
        &mut self,
        input: &mut impl MakeInput,
        description: Option<String>,
    ) -> Result<()> {
        let mut initial = description;
        loop {
            let request = if let Some(s) = initial.take() {
                Some(s)
            } else {
                input.read_request("Describe a workflow change, /show, or /accept:")?
            };
            let Some(request) = request else {
                self.log("cancelled", "cancelled")?;
                break;
            };
            match request.trim() {
                "" => (),
                "/show" => match std::fs::read_to_string(&self.draft) {
                    Ok(s) => crate::display::print_bordered(&s, Some("Workflow draft")),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => println!("No draft yet."),
                    Err(e) => return Err(e.into()),
                },
                "/accept" => {
                    if !self.draft.exists() {
                        println!("No draft yet.");
                        continue;
                    }
                    loop {
                        match input.choose_approval(&self.dest)? {
                            Approval::Continue => break,
                            Approval::Cancel => {
                                self.log("cancelled", "cancelled")?;
                                println!("Staging: {}", self.staging.display());
                                return Ok(());
                            }
                            Approval::Write => match self.accept() {
                                Ok(()) => {
                                    println!("Staging: {}", self.staging.display());
                                    return Ok(());
                                }
                                Err(e) => eprintln!("{e}"),
                            },
                        }
                    }
                }
                _ => {
                    if let Err(e) = self.submit(&request).await {
                        eprintln!("{e}");
                    }
                }
            }
        }
        println!("Staging: {}", self.staging.display());
        Ok(())
    }
}

pub(crate) fn require_interactive_terminal(stdin_tty: bool, stdout_tty: bool) -> Result<()> {
    if !stdin_tty || !stdout_tty {
        return Err(CruiseError::Other(
            "cruise workflow make requires an interactive terminal (stdin and stdout); use `cruise workflow generate` for non-interactive drafting".into(),
        ));
    }
    Ok(())
}

pub(crate) async fn run(args: WorkflowMakeArgs) -> Result<()> {
    run_with_input(
        args,
        &mut TerminalMakeInput,
        std::io::stdin().is_terminal(),
        std::io::stdout().is_terminal(),
    )
    .await
}

fn create_staging_dir(root: &Path, id: &str) -> Result<PathBuf> {
    let mut staging = root.join(id);
    let mut suffix = 0usize;
    loop {
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        match builder.create(&staging) {
            Ok(()) => return Ok(staging),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                suffix += 1;
                staging = root.join(format!("{id}-{suffix}"));
            }
            Err(e) => return Err(e.into()),
        }
    }
}

pub(crate) async fn run_with_input(
    args: WorkflowMakeArgs,
    input: &mut impl MakeInput,
    stdin_tty: bool,
    stdout_tty: bool,
) -> Result<()> {
    require_interactive_terminal(stdin_tty, stdout_tty)?;
    let cwd = std::env::current_dir()?;
    let dest = destination_path(&args.name, args.user, &cwd)?;
    if dest.exists() {
        return Err(CruiseError::Other(format!(
            "workflow already exists: {}",
            dest.display()
        )));
    }
    let (yaml, source) = crate::resolver::resolve_config(args.config.as_deref())?;
    let config = crate::resolver::load_config_from_source(&yaml, &source)?;
    let reference = args
        .from
        .as_deref()
        .map(|s| crate::resolver::resolve_config(Some(s)).map(|(yaml, _)| yaml))
        .transpose()?;
    let root = crate::paths::data_dir()?.join("workflow-make");
    std::fs::create_dir_all(&root)?;
    let id = crate::session::current_timestamp_id();
    let staging = create_staging_dir(&root, &id)?;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(staging.join("conversation.jsonl"))?;
    let draft = staging.join(format!("{}.yaml", args.name));
    Conversation {
        config,
        cwd,
        dest,
        staging,
        draft,
        reference,
        turns: Vec::new(),
    }
    .interact(input, args.description)
    .await
}

#[cfg(all(test, unix))]
#[expect(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::test_support::workflow_authoring::backend_config;
    use std::collections::VecDeque;
    use std::fs;

    const VALID: &str = "command: [cat]\nsteps:\n  build:\n    command: echo hi\n";

    #[test]
    fn staging_is_private_under_permissive_umask() {
        use std::os::unix::fs::PermissionsExt;

        const CHILD_ROOT: &str = "CRUISE_TEST_STAGING_PRIVATE_ROOT";
        if let Some(root) = std::env::var_os(CHILD_ROOT) {
            let root = Path::new(&root);
            let first = create_staging_dir(root, "session").unwrap();
            let second = create_staging_dir(root, "session").unwrap();
            assert_eq!(second, root.join("session-1"));
            for path in [first, second] {
                assert_eq!(fs::metadata(path).unwrap().permissions().mode() & 0o077, 0);
            }
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let status = std::process::Command::new("sh")
            .arg("-c")
            .arg("umask 000; exec \"$1\" --exact workflow_make::tests::staging_is_private_under_permissive_umask")
            .arg("sh")
            .arg(std::env::current_exe().unwrap())
            .env(CHILD_ROOT, root.path())
            .status()
            .unwrap();
        assert!(status.success());
    }

    #[tokio::test]
    async fn claude_make_turns_and_repairs_do_not_override_subagent_model()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        use crate::test_support::{EnvGuard, lock_process, set_fake_home, write_executable_script};

        let _lock = lock_process();
        let temp = tempfile::tempdir()?;
        let _home = set_fake_home(temp.path());
        let _path = crate::test_support::install_exiting_jcode_stub(temp.path());
        let _jcode_home = EnvGuard::set("JCODE_HOME", temp.path().join("jcode-home"));
        let _sdk = EnvGuard::remove("CRUISE_SDK");
        let _model = EnvGuard::remove("CRUISE_MODEL");
        let _plan_model = EnvGuard::remove("CRUISE_PLAN_MODEL");
        let _subagent_model = EnvGuard::remove("CLAUDE_CODE_SUBAGENT_MODEL");
        let _subagent_force = EnvGuard::remove("CLAUDE_CODE_SUBAGENT_MODEL_FORCE");

        let project = temp.path().join("project");
        let staging = temp.path().join("staging");
        fs::create_dir_all(&project)?;
        fs::create_dir_all(&staging)?;
        for (name, text) in [
            (
                "broken",
                "<cruise-workflow>\nsteps: [\n</cruise-workflow>".to_owned(),
            ),
            (
                "valid",
                format!("Draft ready.\n<cruise-workflow>\n{VALID}</cruise-workflow>"),
            ),
        ] {
            let frame = serde_json::json!({"type":"assistant","session_id":"make-sdk","message":{"role":"assistant","content":[{"type":"text","text":text}]}});
            fs::write(temp.path().join(name), format!("{frame}\n"))?;
        }
        let script = format!(
            r#"#!/bin/sh
case "$1" in --version) echo '2.0.0'; exit 0;; --cruise-exec-probe) exit 0;; esac
d='{}'
printf '%s\n' "$@" >> "$d/args"
printf 'model=%s force=%s\n' "${{CLAUDE_CODE_SUBAGENT_MODEL-unset}}" "${{CLAUDE_CODE_SUBAGENT_MODEL_FORCE-unset}}" >> "$d/subagent-env"
read -r input
printf '%s\n' "$input" >> "$d/prompts"
if [ ! -f "$d/attempted" ]; then
  touch "$d/attempted"
  cat "$d/broken"
else
  cat "$d/valid"
fi
printf '%s\n' '{{"type":"result","subtype":"success","is_error":false,"session_id":"make-sdk"}}'
"#,
            temp.path().display()
        );
        write_executable_script(&temp.path().join("claude"), &script);
        let config = WorkflowConfig::from_yaml(
            "sdk: jcode\nsubagent_model: {sdk: claude, model: haiku}\nplan_model: []\nmodel:\n  - sdk: claude\n    model: mixed-yaml-model\n  - unused/jcode\nsteps:\n  build:\n    command: echo hi\n",
        )?;
        let mut conversation = Conversation {
            config,
            cwd: project.clone(),
            dest: project.join(".cruise/ci.yaml"),
            draft: staging.join("ci.yaml"),
            staging,
            reference: None,
            turns: Vec::new(),
        };
        tokio::time::timeout(
            std::time::Duration::from_secs(20),
            conversation.submit("create workflow"),
        )
        .await??;
        tokio::time::timeout(
            std::time::Duration::from_secs(20),
            conversation.submit("review workflow"),
        )
        .await??;

        assert_eq!(
            fs::read_to_string(temp.path().join("subagent-env"))?,
            "model=unset force=unset\n".repeat(3)
        );
        let args = fs::read_to_string(temp.path().join("args"))?;
        assert_eq!(
            args.lines()
                .filter(|line| *line == "mixed-yaml-model")
                .count(),
            3
        );
        let prompts = fs::read_to_string(temp.path().join("prompts"))?;
        assert_eq!(prompts.lines().count(), 3);
        assert!(prompts.contains("Previous attempt"));
        assert!(prompts.contains("steps: ["));
        assert_eq!(
            fs::read_to_string(&conversation.draft)?.trim(),
            VALID.trim()
        );
        assert_eq!(conversation.turns.len(), 4);
        assert!(!conversation.dest.exists());
        Ok(())
    }

    struct Fixture {
        temp: tempfile::TempDir,
        conversation: Conversation,
        calls: PathBuf,
    }

    impl Fixture {
        fn new(script_body: &str) -> Self {
            let temp = tempfile::tempdir().unwrap();
            let cwd = temp.path().join("project");
            let staging = temp.path().join("staging");
            fs::create_dir_all(&cwd).unwrap();
            fs::create_dir_all(&staging).unwrap();
            let calls = temp.path().join("calls");
            let script = temp.path().join("backend.sh");
            crate::test_support::write_executable_script(
                &script,
                &format!(
                    "#!/bin/sh\nprintf x >> '{}'\n{}\n",
                    calls.display(),
                    script_body
                ),
            );
            let config = backend_config(&script);
            let conversation = Conversation {
                config,
                dest: cwd.join(".cruise/ci.yaml"),
                cwd,
                draft: staging.join("ci.yaml"),
                staging,
                reference: None,
                turns: Vec::new(),
            };
            Self {
                temp,
                conversation,
                calls,
            }
        }

        fn events(&self) -> Vec<String> {
            fs::read_to_string(self.conversation.staging.join("conversation.jsonl"))
                .unwrap()
                .lines()
                .map(|line| {
                    serde_json::from_str::<serde_json::Value>(line).unwrap()["event"]
                        .as_str()
                        .unwrap()
                        .to_owned()
                })
                .collect()
        }
        fn count(&self) -> usize {
            fs::read(&self.calls).map_or(0, |s| s.len())
        }
    }

    enum Choice {
        Plain(Approval),
        TamperDraft(PathBuf, String),
        CreateDestination(String),
    }
    struct Scripted {
        requests: VecDeque<Option<String>>,
        choices: VecDeque<Choice>,
        approval_calls: usize,
    }
    impl Scripted {
        fn new(requests: &[Option<&str>], choices: Vec<Choice>) -> Self {
            Self {
                requests: requests.iter().map(|r| r.map(str::to_owned)).collect(),
                choices: choices.into(),
                approval_calls: 0,
            }
        }
    }
    impl MakeInput for Scripted {
        fn read_request(&mut self, _prompt: &str) -> Result<Option<String>> {
            self.requests
                .pop_front()
                .ok_or_else(|| CruiseError::Other("unexpected request prompt".into()))
        }
        fn choose_approval(&mut self, dest: &Path) -> Result<Approval> {
            self.approval_calls += 1;
            Ok(
                match self
                    .choices
                    .pop_front()
                    .ok_or_else(|| CruiseError::Other("unexpected approval prompt".into()))?
                {
                    Choice::Plain(choice) => choice,
                    Choice::TamperDraft(path, content) => {
                        fs::write(path, content)?;
                        Approval::Write
                    }
                    Choice::CreateDestination(content) => {
                        fs::create_dir_all(dest.parent().unwrap())?;
                        fs::write(dest, content)?;
                        Approval::Write
                    }
                },
            )
        }
    }

    #[tokio::test]
    async fn approval_revalidates_tampered_draft_and_returns_to_menu() {
        let mut f = Fixture::new("exit 1");
        fs::write(&f.conversation.draft, VALID).unwrap();
        let mut input = Scripted::new(
            &[Some("/accept")],
            vec![
                Choice::TamperDraft(f.conversation.draft.clone(), "steps: [\n".into()),
                Choice::Plain(Approval::Cancel),
            ],
        );
        f.conversation.interact(&mut input, None).await.unwrap();
        assert_eq!(input.approval_calls, 2);
        assert!(!f.conversation.dest.exists());
        assert_eq!(
            fs::read_to_string(&f.conversation.draft).unwrap(),
            "steps: [\n"
        );
        assert_eq!(f.events(), ["cancelled"]);
        assert_eq!(f.count(), 0);
    }

    #[tokio::test]
    async fn destination_created_at_approval_is_not_overwritten() {
        let mut f = Fixture::new("exit 1");
        fs::write(&f.conversation.draft, VALID).unwrap();
        let mut input = Scripted::new(
            &[Some("/accept")],
            vec![
                Choice::CreateDestination("existing content".into()),
                Choice::Plain(Approval::Cancel),
            ],
        );
        f.conversation.interact(&mut input, None).await.unwrap();
        assert_eq!(input.approval_calls, 2);
        assert_eq!(
            fs::read_to_string(&f.conversation.dest).unwrap(),
            "existing content"
        );
        assert_eq!(fs::read_to_string(&f.conversation.draft).unwrap(), VALID);
        assert_eq!(f.events(), ["cancelled"]);
    }

    #[tokio::test]
    async fn no_draft_does_not_open_approval_or_call_backend() {
        let mut f = Fixture::new("exit 1");
        let mut input = Scripted::new(&[Some("/accept"), None], vec![]);
        f.conversation.interact(&mut input, None).await.unwrap();
        assert_eq!(input.approval_calls, 0);
        assert_eq!(f.count(), 0);
        assert!(!f.conversation.dest.exists());
        assert_eq!(f.events(), ["cancelled"]);
    }

    #[tokio::test]
    async fn continue_returns_to_input_then_can_write() {
        let mut f = Fixture::new("exit 1");
        fs::write(&f.conversation.draft, VALID).unwrap();
        let mut input = Scripted::new(
            &[Some("/accept"), Some("/accept")],
            vec![
                Choice::Plain(Approval::Continue),
                Choice::Plain(Approval::Write),
            ],
        );
        f.conversation.interact(&mut input, None).await.unwrap();
        assert_eq!(input.approval_calls, 2);
        assert_eq!(
            fs::read(&f.conversation.dest).unwrap(),
            fs::read(&f.conversation.draft).unwrap()
        );
        assert_eq!(f.events(), ["accepted"]);
        assert_eq!(f.count(), 0);
    }

    #[tokio::test]
    async fn draft_log_failure_keeps_committed_draft_and_transcript() {
        let mut f = Fixture::new("exit 1");
        let log = f.conversation.staging.join("conversation.jsonl");
        crate::test_support::write_executable_script(
            &f.temp.path().join("backend.sh"),
            &format!(
                "#!/bin/sh\ncat >/dev/null\nrm '{}'\nmkdir '{}'\nprintf 'Ready\\n<cruise-workflow>\\n{}\\n</cruise-workflow>\\n'\n",
                log.display(),
                log.display(),
                VALID
            ),
        );
        f.conversation.submit("create it").await.unwrap();
        assert_eq!(
            fs::read_to_string(&f.conversation.draft).unwrap().trim(),
            VALID.trim()
        );
        assert_eq!(f.conversation.turns.len(), 2);
        assert_eq!(f.conversation.turns[0].text, "create it");
        assert_eq!(f.conversation.turns[1].text, "Ready");
    }

    #[tokio::test]
    async fn acceptance_log_failure_exits_without_second_approval() {
        let mut f = Fixture::new("exit 1");
        fs::write(&f.conversation.draft, VALID).unwrap();
        fs::create_dir(f.conversation.staging.join("conversation.jsonl")).unwrap();
        let mut input = Scripted::new(&[Some("/accept")], vec![Choice::Plain(Approval::Write)]);
        f.conversation.interact(&mut input, None).await.unwrap();
        assert_eq!(input.approval_calls, 1);
        assert_eq!(fs::read_to_string(&f.conversation.dest).unwrap(), VALID);
    }

    #[tokio::test]
    async fn backend_failure_preserves_draft_and_conversation_continues() {
        let mut f = Fixture::new("cat >/dev/null\necho failure >&2\nexit 1");
        fs::write(&f.conversation.draft, VALID).unwrap();
        let mut input = Scripted::new(&[Some("try a change"), None], vec![]);
        f.conversation.interact(&mut input, None).await.unwrap();
        assert_eq!(f.count(), 1);
        assert_eq!(fs::read_to_string(&f.conversation.draft).unwrap(), VALID);
        assert!(f.conversation.turns.is_empty());
        assert_eq!(f.events(), ["request", "backend_error", "cancelled"]);
        assert!(!f.conversation.dest.exists());
    }

    #[tokio::test]
    async fn read_only_violation_rejects_turn_without_repair() {
        let mut f = Fixture::new(
            "cat >/dev/null\nprintf changed > forbidden.txt\nprintf '<cruise-workflow>\\nsteps: {}\\n</cruise-workflow>\\n'",
        );
        fs::write(&f.conversation.draft, VALID).unwrap();
        let err = f.conversation.submit("change workflow").await.unwrap_err();
        assert!(
            matches!(err, CruiseError::ReadOnlyWorkspaceChanged),
            "{err:?}"
        );
        assert_eq!(f.count(), 1);
        assert_eq!(fs::read_to_string(&f.conversation.draft).unwrap(), VALID);
        assert!(f.conversation.turns.is_empty());
        assert_eq!(f.events(), ["request", "backend_error"]);
        assert!(!f.conversation.dest.exists());
    }

    #[test]
    fn reply_protocol_handles_messages_blocks_and_invalid_tags() {
        assert_eq!(parse_reply("Which tool?").unwrap().workflow, None);
        let reply = parse_reply(
            "Before\n <cruise-workflow> \n\n```yaml\nsteps: {}\n```\n</cruise-workflow>\nAfter",
        )
        .unwrap();
        assert_eq!(reply.message, "Before\nAfter");
        assert_eq!(reply.workflow.as_deref(), Some("steps: {}"));
        for input in [
            "",
            "steps: {}",
            "<cruise-workflow>",
            "</cruise-workflow>\n<cruise-workflow>",
            "<cruise-workflow>\n\n</cruise-workflow>",
            "<cruise-workflow>\na\n</cruise-workflow>\n<cruise-workflow>\nb\n</cruise-workflow>",
        ] {
            assert!(parse_reply(input).is_err(), "{input:?}");
        }
    }

    #[test]
    fn substitutions_do_not_reprocess_user_content() {
        let prompt = build_turn_prompt(
            TEMPLATE,
            SCHEMA,
            Some(("reference", "{request}")),
            "{draft}",
            Some("{schema}"),
            "{reference}",
            None,
        );
        assert!(prompt.contains("## Reference workflow\nreference\n{request}"));
        assert!(prompt.contains("## Earlier conversation\n{draft}"));
        assert!(prompt.contains("## Current draft\n{schema}"));
        assert!(prompt.contains("## Request\n{reference}"));
    }

    #[test]
    fn production_prompt_includes_repair_context_only_when_requested() {
        let plain = build_turn_prompt(TEMPLATE, "schema", None, "history", None, "request", None);
        assert!(!plain.contains("## Previous attempt"));
        let repair = build_turn_prompt(
            TEMPLATE,
            "schema",
            None,
            "history",
            None,
            "request",
            Some(("unique reply", "unique diagnostic")),
        );
        assert_eq!(repair.matches("unique reply").count(), 1);
        assert_eq!(repair.matches("unique diagnostic").count(), 1);
        assert_eq!(repair.matches("## Previous attempt").count(), 1);
    }

    #[test]
    fn transcript_omits_oldest_whole_turns_at_byte_budget() {
        let turns = vec![
            Turn {
                role: Role::User,
                text: "old".into(),
            },
            Turn {
                role: Role::Assistant,
                text: "\u{65b0}".into(),
            },
        ];
        let newest = format!("Assistant: {}\n", turns[1].text);
        let rendered = render_transcript(&turns, newest.len());
        assert!(rendered.starts_with("(earlier turns omitted: 1)"));
        assert!(rendered.contains(&newest));
        assert!(!rendered.contains("old"));
        assert_eq!(render_transcript(&[], 0), "(no earlier turns)");
    }

    #[test]
    fn transcript_skips_oversized_newest_and_retains_older() {
        let turns = vec![
            Turn {
                role: Role::User,
                text: "fits".into(),
            },
            Turn {
                role: Role::Assistant,
                text: "too large".repeat(20),
            },
        ];
        assert_eq!(
            render_transcript(&turns, "User: fits\n".len()),
            "(earlier turns omitted: 1)\nUser: fits\n"
        );

        let turns = vec![
            Turn {
                role: Role::Assistant,
                text: "\u{65b0}".into(),
            },
            Turn {
                role: Role::User,
                text: "\u{5927}".repeat(20),
            },
        ];
        assert_eq!(
            render_transcript(&turns, "Assistant: \u{65b0}\n".len()),
            "(earlier turns omitted: 1)\nAssistant: \u{65b0}\n"
        );
        assert_eq!(
            render_transcript(&turns, "Assistant: \u{65b0}\n".len() - 1),
            "(earlier turns omitted: 2)\n"
        );
    }
}
