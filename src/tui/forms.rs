use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::event::KeyEvent;
use ratatui_textarea::TextArea;

use crate::application::NewSessionRequest;
use crate::new_session_draft::NewSessionDraft;
use crate::session::WorkspaceMode;

/// Editable text control backed by ratatui-textarea.  Keeping this wrapper
/// small lets the form remain a deterministic, testable value object.
pub struct Editor {
    area: TextArea<'static>,
}

impl Default for Editor {
    fn default() -> Self {
        Self::new("")
    }
}

impl Editor {
    #[must_use]
    pub fn new(value: &str) -> Self {
        Self {
            area: TextArea::new(value.split('\n').map(ToString::to_string).collect()),
        }
    }
    pub fn input(&mut self, event: KeyEvent) {
        self.area.input(event);
    }
    #[must_use]
    pub fn text(&self) -> String {
        self.area.lines().join("\n")
    }
    pub fn set_text(&mut self, value: &str) {
        *self = Self::new(value);
    }
    #[must_use]
    pub fn widget(&self) -> &TextArea<'static> {
        &self.area
    }
    /// Replace the text with the neighbour of the current value in `values`,
    /// wrapping at both ends.  An unknown current value starts at the first entry.
    fn cycle<T, F>(&mut self, values: &[T], delta: isize, text: F)
    where
        F: for<'a> Fn(&'a T) -> &'a str,
    {
        if values.is_empty() {
            return;
        }
        let current = self.text();
        let current = current.trim();
        let len = values.len().cast_signed();
        let index = values
            .iter()
            .position(|value| text(value) == current)
            .map_or(0, |index| {
                (index.cast_signed() + delta)
                    .rem_euclid(len)
                    .cast_unsigned()
            });
        self.set_text(text(&values[index]));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    Directory,
    GitHub,
}

/// One question of the New Session dialogue.  Questions are asked in
/// [`Step::ORDER`]; [`NewSessionForm::applies`] hides the ones that earlier
/// answers make moot (for example the working directory of a GitHub clone).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Task,
    Attachments,
    Source,
    WorkingDirectory,
    Repository,
    Config,
    SkippedSteps,
    Workspace,
    DirtyTree,
    FormalSpec,
    Launch,
}

impl Step {
    pub const ORDER: [Self; 11] = [
        Self::Task,
        Self::Attachments,
        Self::Source,
        Self::WorkingDirectory,
        Self::Repository,
        Self::Config,
        Self::SkippedSteps,
        Self::Workspace,
        Self::DirtyTree,
        Self::FormalSpec,
        Self::Launch,
    ];

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Task => "Task",
            Self::Attachments => "Images",
            Self::Source => "Source",
            Self::WorkingDirectory => "Working directory",
            Self::Repository => "GitHub repository",
            Self::Config => "Workflow config",
            Self::SkippedSteps => "Skipped steps",
            Self::Workspace => "Workspace",
            Self::DirtyTree => "Dirty working tree",
            Self::FormalSpec => "Formal specification",
            Self::Launch => "Launch",
        }
    }

    /// Answered by typing into an editor.  Skipped steps are also typed when
    /// the workflow config offers no step list; the reducer decides that.
    #[must_use]
    pub fn is_text(self) -> bool {
        matches!(
            self,
            Self::Task
                | Self::Attachments
                | Self::WorkingDirectory
                | Self::Repository
                | Self::Config
        )
    }
    /// Text steps where Enter inserts a newline; Tab or Ctrl+Enter moves on.
    #[must_use]
    pub fn is_multiline(self) -> bool {
        matches!(self, Self::Task | Self::Attachments)
    }
    /// Answered by picking from a fixed list with ↑↓ or Space.
    #[must_use]
    pub fn is_choice(self) -> bool {
        matches!(
            self,
            Self::Source | Self::Workspace | Self::DirtyTree | Self::FormalSpec | Self::Launch
        )
    }
}

/// How the dialogue ends: which planning mode starts, or whether the answers
/// are only kept as a draft session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Launch {
    Planning,
    Grill,
    InputPlan,
    SaveDraft,
}

impl Launch {
    pub const ALL: [Self; 4] = [
        Self::Planning,
        Self::Grill,
        Self::InputPlan,
        Self::SaveDraft,
    ];

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Planning => "Start planning",
            Self::Grill => "Grill planning (interview first)",
            Self::InputPlan => "Use the input as the plan (no LLM planning)",
            Self::SaveDraft => "Save as draft",
        }
    }
    #[must_use]
    pub fn shortcut(self) -> &'static str {
        match self {
            Self::Planning => "Ctrl+P",
            Self::Grill => "Ctrl+G",
            Self::InputPlan => "Ctrl+U",
            Self::SaveDraft => "Ctrl+S",
        }
    }
}

/// State for the New Session dialogue.  Only the draft fields persist; the
/// current step and provider toggles are intentionally ephemeral.
pub struct NewSessionForm {
    pub input: Editor,
    pub working_dir: Editor,
    pub repository: Editor,
    pub config: Editor,
    pub attachments: Editor,
    pub skipped: Editor,
    pub step: Step,
    pub source: SourceKind,
    pub workspace_mode: WorkspaceMode,
    pub launch: Launch,
    pub options: SessionOptions,
    pub skipped_explicit: bool,
    pub dirty: bool,
    pub last_change: Instant,
}

#[derive(Default)]
pub struct SessionOptions {
    pub allow_dirty_working_tree: bool,
    pub planning: PlanningFlags,
}

#[derive(Default)]
pub struct PlanningFlags {
    pub grill: bool,
    pub formal_spec: bool,
    pub skip_planning: bool,
}

impl Default for NewSessionForm {
    fn default() -> Self {
        Self::from_draft(None)
    }
}
impl NewSessionForm {
    #[must_use]
    pub fn from_draft(draft: Option<&NewSessionDraft>) -> Self {
        let draft = draft.cloned().unwrap_or_default();
        Self {
            input: Editor::new(&draft.input),
            working_dir: Editor::new(&draft.working_dir),
            repository: Editor::new(draft.repo.as_deref().unwrap_or("")),
            config: Editor::new(draft.requested_config_path.as_deref().unwrap_or("")),
            attachments: Editor::default(),
            skipped: Editor::new(&draft.skipped_steps.join(", ")),
            step: Step::Task,
            source: if draft.repo.is_some() {
                SourceKind::GitHub
            } else {
                SourceKind::Directory
            },
            workspace_mode: WorkspaceMode::Worktree,
            launch: Launch::Planning,
            options: SessionOptions::default(),
            skipped_explicit: !draft.skipped_steps.is_empty(),
            dirty: false,
            last_change: Instant::now(),
        }
    }

    pub fn mark_changed(&mut self) {
        self.dirty = true;
        self.last_change = Instant::now();
    }
    #[must_use]
    pub fn should_autosave(&self, now: Instant) -> bool {
        self.dirty && now.duration_since(self.last_change) >= Duration::from_millis(500)
    }
    pub fn mark_saved(&mut self) {
        self.dirty = false;
    }
    #[must_use]
    pub fn selected_skipped_steps(&self) -> Vec<String> {
        self.skipped
            .text()
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
            .collect()
    }
    #[must_use]
    pub fn attachment_paths(&self) -> Vec<PathBuf> {
        self.attachments
            .text()
            .lines()
            .filter_map(|line| {
                let path = line.trim();
                (!path.is_empty())
                    .then(|| PathBuf::from(crate::new_session_history::expand_tilde(path)))
            })
            .collect()
    }

    #[must_use]
    pub fn draft(&self) -> NewSessionDraft {
        NewSessionDraft {
            input: self.input.text(),
            requested_config_path: nonempty(&self.config.text()),
            working_dir: self.working_dir.text(),
            repo: (self.source == SourceKind::GitHub)
                .then(|| nonempty(&self.repository.text()))
                .flatten(),
            skipped_steps: self.selected_skipped_steps(),
            updated_at: String::new(),
        }
    }

    #[must_use]
    pub fn request(&self) -> NewSessionRequest {
        let base_dir = self.working_dir.text().trim().to_string();
        let config_path = nonempty(&self.config.text())
            .map(|path| PathBuf::from(crate::new_session_history::expand_tilde(&path)));
        let repo = (self.source == SourceKind::GitHub)
            .then(|| nonempty(&self.repository.text()))
            .flatten();
        NewSessionRequest {
            input: self.input.text(),
            base_dir: PathBuf::from(crate::new_session_history::expand_tilde(
                if base_dir.is_empty() { "." } else { &base_dir },
            )),
            config_path,
            config_source: None,
            config_yaml: None,
            repo,
            workspace_mode: if self.source == SourceKind::GitHub {
                WorkspaceMode::Worktree
            } else {
                self.workspace_mode
            },
            allow_dirty_working_tree: self.options.allow_dirty_working_tree,
            attachments: self.attachment_paths(),
            skipped_steps: self.selected_skipped_steps(),
        }
    }

    /// Whether `step` is asked given the answers so far.
    #[must_use]
    pub fn applies(&self, step: Step) -> bool {
        match step {
            Step::WorkingDirectory | Step::Workspace => self.source == SourceKind::Directory,
            Step::Repository => self.source == SourceKind::GitHub,
            Step::DirtyTree => {
                self.source == SourceKind::Directory
                    && self.workspace_mode == WorkspaceMode::CurrentBranch
            }
            _ => true,
        }
    }
    /// The questions of this dialogue, in order.
    pub fn steps(&self) -> impl Iterator<Item = Step> + '_ {
        Step::ORDER.into_iter().filter(|step| self.applies(*step))
    }
    /// Move to the next question; `false` when the dialogue is at its last one.
    pub fn advance(&mut self) -> bool {
        let current = self.step;
        let next = self.steps().skip_while(|step| *step != current).nth(1);
        if let Some(next) = next {
            self.step = next;
            true
        } else {
            false
        }
    }
    /// Move back to the previous question; `false` when already at the first.
    pub fn retreat(&mut self) -> bool {
        let current = self.step;
        let previous = self.steps().take_while(|step| *step != current).last();
        if let Some(previous) = previous {
            self.step = previous;
            true
        } else {
            false
        }
    }
    pub fn rewind(&mut self) {
        self.step = Step::Task;
    }

    pub fn toggle_workspace(&mut self) {
        self.workspace_mode = match self.workspace_mode {
            WorkspaceMode::Worktree => WorkspaceMode::CurrentBranch,
            WorkspaceMode::CurrentBranch => WorkspaceMode::Worktree,
        };
        self.mark_changed();
    }
    /// Change the answer of the current choice step by `delta` positions.
    /// Two-way choices toggle; text steps are unaffected.
    pub fn choose(&mut self, delta: isize) {
        match self.step {
            Step::Source => {
                self.source = match self.source {
                    SourceKind::Directory => SourceKind::GitHub,
                    SourceKind::GitHub => SourceKind::Directory,
                };
                if self.source == SourceKind::GitHub {
                    self.workspace_mode = WorkspaceMode::Worktree;
                }
                self.mark_changed();
            }
            Step::Workspace => self.toggle_workspace(),
            Step::DirtyTree => {
                self.options.allow_dirty_working_tree = !self.options.allow_dirty_working_tree;
                self.mark_changed();
            }
            Step::FormalSpec => {
                self.options.planning.formal_spec = !self.options.planning.formal_spec;
                self.options.planning.skip_planning &= !self.options.planning.formal_spec;
                self.mark_changed();
            }
            Step::Launch => {
                let len = Launch::ALL.len().cast_signed();
                let index = Launch::ALL
                    .iter()
                    .position(|launch| *launch == self.launch)
                    .map_or(0, |index| {
                        (index.cast_signed() + delta)
                            .rem_euclid(len)
                            .cast_unsigned()
                    });
                self.launch = Launch::ALL[index];
            }
            _ => {}
        }
    }
    /// Record the launch mode in the planning flags carried by the request.
    pub fn select_launch(&mut self, launch: Launch) {
        self.launch = launch;
        let planning = &mut self.options.planning;
        planning.grill = launch == Launch::Grill;
        planning.skip_planning = launch == Launch::InputPlan;
        planning.formal_spec &= !planning.skip_planning;
    }
    pub fn input(&mut self, event: KeyEvent) {
        match self.step {
            Step::Task => self.input.input(event),
            Step::WorkingDirectory => self.working_dir.input(event),
            Step::Repository => self.repository.input(event),
            Step::Config => self.config.input(event),
            Step::Attachments => self.attachments.input(event),
            Step::SkippedSteps => self.skipped.input(event),
            _ => return,
        }
        self.mark_changed();
        if self.step == Step::SkippedSteps {
            self.skipped_explicit = true;
        }
    }

    /// The answer shown once a step has been passed.
    #[must_use]
    pub fn answer(&self, step: Step) -> String {
        match step {
            Step::Task => {
                let text = self.input.text();
                let mut lines = text.lines().map(str::trim).filter(|line| !line.is_empty());
                match (lines.next(), lines.next()) {
                    (None, _) => "(none)".to_string(),
                    (Some(first), None) => first.to_string(),
                    (Some(first), Some(_)) => format!("{first} …"),
                }
            }
            Step::Attachments => match self.attachment_paths().len() {
                0 => "none".to_string(),
                1 => "1 image".to_string(),
                count => format!("{count} images"),
            },
            Step::Source => match self.source {
                SourceKind::Directory => "Directory",
                SourceKind::GitHub => "GitHub repository",
            }
            .to_string(),
            Step::WorkingDirectory => nonempty(&self.working_dir.text())
                .unwrap_or_else(|| "current directory".to_string()),
            Step::Repository => self.repository.text().trim().to_string(),
            Step::Config => {
                nonempty(&self.config.text()).unwrap_or_else(|| "auto-detect".to_string())
            }
            Step::SkippedSteps => {
                let skipped = self.selected_skipped_steps();
                if skipped.is_empty() {
                    "none".to_string()
                } else {
                    skipped.join(", ")
                }
            }
            Step::Workspace => match self.workspace_mode {
                WorkspaceMode::Worktree => "worktree",
                WorkspaceMode::CurrentBranch => "current branch",
            }
            .to_string(),
            Step::DirtyTree => yes_no(self.options.allow_dirty_working_tree).to_string(),
            Step::FormalSpec => yes_no(self.options.planning.formal_spec).to_string(),
            Step::Launch => self.launch.label().to_string(),
        }
    }

    pub fn apply_history_defaults(
        &mut self,
        summary: &crate::application::NewSessionHistorySummary,
    ) {
        if self.working_dir.text().trim().is_empty()
            && let Some(path) = summary.last_working_dir.as_deref()
        {
            self.working_dir.set_text(path);
        }
        if self.config.text().trim().is_empty()
            && let Some(path) = summary.last_requested_config_path.as_deref()
        {
            self.config.set_text(path);
        }
    }
    /// Apply history-derived skipped steps without turning them into an
    /// explicit user choice.  A later edit/toggle marks the field explicit.
    pub fn apply_default_skips(&mut self, skipped_steps: &[String]) {
        if !self.skipped_explicit {
            self.skipped.set_text(&skipped_steps.join(", "));
        }
    }

    /// Recall recent working directories recorded by session history.
    pub fn cycle_working_directory(&mut self, paths: &[String], delta: isize) {
        if paths.is_empty() {
            return;
        }
        self.working_dir.cycle(paths, delta, |path| path);
        self.mark_changed();
    }

    pub fn cycle_config(&mut self, sources: &[crate::resolver::ConfigCandidate], delta: isize) {
        if sources.is_empty() {
            return;
        }
        let values = sources
            .iter()
            .map(crate::resolver::ConfigCandidate::selection_value)
            .fold(Vec::<String>::new(), |mut values, value| {
                if !values.iter().any(|existing| existing == &value) {
                    values.push(value);
                }
                values
            });
        let current = self.config.text();
        let current = current.trim();
        let current_index = if current.is_empty() {
            0
        } else {
            values
                .iter()
                .position(|value| value == current)
                .map_or(0, |index| index + 1)
        };
        let length = values.len().saturating_add(1).cast_signed();
        let index = (current_index.cast_signed() + delta)
            .rem_euclid(length)
            .cast_unsigned();
        if index == 0 {
            self.config.set_text("");
        } else {
            self.config.set_text(&values[index - 1]);
        }
        self.mark_changed();
    }

    /// Recall repositories reported by `gh repo list`.
    pub fn cycle_repository(&mut self, repositories: &[String], delta: isize) {
        if repositories.is_empty() {
            return;
        }
        self.repository
            .cycle(repositories, delta, |repository| repository);
        self.mark_changed();
    }

    /// Check the current answer before moving to the next question.
    pub fn validate_step(&self) -> Result<(), String> {
        match self.step {
            Step::Attachments
                if self.input.text().trim().is_empty() && self.attachment_paths().is_empty() =>
            {
                Err("Task description or an image attachment is required".to_string())
            }
            Step::Repository if self.repository.text().trim().is_empty() => {
                Err("Select a GitHub repository before creating a session".to_string())
            }
            _ => Ok(()),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.options.planning.grill && self.options.planning.skip_planning {
            return Err("Grill planning requires interactive LLM planning".to_string());
        }
        if self.options.planning.formal_spec && self.options.planning.skip_planning {
            return Err("Formal specification requires LLM planning".to_string());
        }
        if self.source == SourceKind::GitHub && self.repository.text().trim().is_empty() {
            return Err("Select a GitHub repository before creating a session".to_string());
        }
        Ok(())
    }
}

fn nonempty(value: &str) -> Option<String> {
    (!value.trim().is_empty()).then(|| value.trim().to_string())
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

#[cfg(test)]
#[expect(
    clippy::field_reassign_with_default,
    reason = "form tests intentionally switch steps and toggle individual controls"
)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn draft_autosaves_only_after_quiet_window() {
        let mut form = NewSessionForm::default();
        form.mark_changed();
        assert!(!form.should_autosave(form.last_change + Duration::from_millis(499)));
        assert!(form.should_autosave(form.last_change + Duration::from_millis(500)));
        form.mark_saved();
        assert!(!form.should_autosave(form.last_change + Duration::from_millis(500)));
    }

    #[test]
    fn request_maps_source_config_skip_images_and_workspace_mode() {
        let mut form = NewSessionForm {
            step: Step::Source,
            ..NewSessionForm::default()
        };
        form.choose(1);
        assert_eq!(form.source, SourceKind::GitHub);
        form.repository.set_text(" acme/cruise ");
        form.config.set_text(" workflow.yaml ");
        form.attachments
            .set_text(" image-a.png\n /tmp/image-b.png\n");
        form.skipped.set_text(" build, , test ");
        form.workspace_mode = WorkspaceMode::Worktree;
        form.toggle_workspace();

        let request = form.request();
        assert_eq!(request.repo.as_deref(), Some("acme/cruise"));
        assert_eq!(request.config_path, Some(PathBuf::from("workflow.yaml")));
        assert_eq!(
            request.attachments,
            vec![
                PathBuf::from("image-a.png"),
                PathBuf::from("/tmp/image-b.png")
            ]
        );
        assert_eq!(
            request.skipped_steps,
            vec!["build".to_string(), "test".to_string()]
        );
        assert_eq!(
            request.workspace_mode,
            WorkspaceMode::Worktree,
            "GitHub sessions always use worktrees"
        );

        form.source = SourceKind::Directory;
        assert_eq!(form.request().workspace_mode, WorkspaceMode::CurrentBranch);
    }

    #[test]
    fn source_and_planning_mode_validation_rejects_invalid_combinations() {
        let mut form = NewSessionForm {
            source: SourceKind::GitHub,
            ..NewSessionForm::default()
        };
        assert_eq!(
            form.validate(),
            Err("Select a GitHub repository before creating a session".to_string())
        );
        form.repository.set_text("acme/cruise");
        assert!(form.validate().is_ok());

        form.options.planning.grill = true;
        form.options.planning.skip_planning = true;
        assert_eq!(
            form.validate(),
            Err("Grill planning requires interactive LLM planning".to_string())
        );
    }

    #[test]
    fn config_cycles_both_ways_and_text_input_marks_form_dirty() {
        let mut form = NewSessionForm::default();
        let sources = [
            crate::resolver::ConfigCandidate {
                label: "A".to_string(),
                source: crate::resolver::CandidateKind::Local(PathBuf::from("a.yaml")),
            },
            crate::resolver::ConfigCandidate {
                label: "B".to_string(),
                source: crate::resolver::CandidateKind::Local(PathBuf::from("b.yaml")),
            },
            crate::resolver::ConfigCandidate {
                label: "Built-in default".to_string(),
                source: crate::resolver::CandidateKind::Builtin,
            },
        ];
        form.cycle_config(&sources, 1);
        assert_eq!(form.config.text(), "a.yaml");
        form.cycle_config(&sources, 1);
        assert_eq!(form.config.text(), "b.yaml");
        form.cycle_config(&sources, -1);
        assert_eq!(form.config.text(), "a.yaml");
        form.cycle_config(&sources, -1);
        assert!(form.config.text().is_empty(), "recall wraps to Auto-detect");

        form.step = Step::SkippedSteps;
        form.input(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        assert!(form.dirty);
        assert!(form.skipped_explicit);
    }

    #[test]
    fn config_cycle_includes_auto_and_wraps_in_both_directions() {
        let mut form = NewSessionForm::default();
        let sources = [
            crate::resolver::ConfigCandidate {
                label: "first.yaml".to_string(),
                source: crate::resolver::CandidateKind::Local(PathBuf::from("first.yaml")),
            },
            crate::resolver::ConfigCandidate {
                label: "second.yaml".to_string(),
                source: crate::resolver::CandidateKind::Local(PathBuf::from("second.yaml")),
            },
            crate::resolver::ConfigCandidate {
                label: "Built-in default".to_string(),
                source: crate::resolver::CandidateKind::Builtin,
            },
        ];

        // Down starts at Auto-detect and visits every CLI candidate before wrapping.
        form.cycle_config(&sources, 1);
        assert_eq!(form.config.text(), "first.yaml");
        form.cycle_config(&sources, 1);
        assert_eq!(form.config.text(), "second.yaml");
        form.cycle_config(&sources, 1);
        assert_eq!(
            form.config.text(),
            crate::new_session_history::BUILTIN_CONFIG_KEY
        );
        form.cycle_config(&sources, 1);
        assert!(
            form.config.text().is_empty(),
            "cycle should wrap to Auto-detect"
        );

        // Up from Auto-detect wraps in the opposite direction through Built-in.
        form.cycle_config(&sources, -1);
        assert_eq!(
            form.config.text(),
            crate::new_session_history::BUILTIN_CONFIG_KEY
        );
        form.cycle_config(&sources, -1);
        assert_eq!(form.config.text(), "second.yaml");
        form.cycle_config(&sources, -1);
        assert_eq!(form.config.text(), "first.yaml");
        form.cycle_config(&sources, -1);
        assert!(
            form.config.text().is_empty(),
            "cycle should wrap back to Auto-detect"
        );
    }

    #[test]
    fn config_cycle_skips_duplicate_paths_from_environment_and_local_candidates() {
        let mut form = NewSessionForm::default();
        let sources = [
            crate::resolver::ConfigCandidate {
                label: "CRUISE_CONFIG → ./cruise.yaml".to_string(),
                source: crate::resolver::CandidateKind::EnvVar(PathBuf::from(
                    "/project/cruise.yaml",
                )),
            },
            crate::resolver::ConfigCandidate {
                label: "./cruise.yaml".to_string(),
                source: crate::resolver::CandidateKind::Local(PathBuf::from(
                    "/project/cruise.yaml",
                )),
            },
            crate::resolver::ConfigCandidate {
                label: "Built-in default".to_string(),
                source: crate::resolver::CandidateKind::Builtin,
            },
        ];

        form.cycle_config(&sources, 1);
        assert_eq!(form.config.text(), "/project/cruise.yaml");
        form.cycle_config(&sources, 1);
        assert_eq!(
            form.config.text(),
            crate::new_session_history::BUILTIN_CONFIG_KEY
        );
        form.cycle_config(&sources, 1);
        assert!(form.config.text().is_empty());
    }

    #[test]
    fn config_auto_and_builtin_values_round_trip_to_draft_and_request() {
        let mut form = NewSessionForm::default();

        // Blank is the only value that means Auto-detect.
        assert_eq!(form.draft().requested_config_path, None);
        assert_eq!(form.request().config_path, None);

        // Built-in is persisted as the existing sentinel, not as Auto-detect.
        form.config
            .set_text(crate::new_session_history::BUILTIN_CONFIG_KEY);
        assert_eq!(
            form.draft().requested_config_path.as_deref(),
            Some(crate::new_session_history::BUILTIN_CONFIG_KEY)
        );
        assert_eq!(
            form.request().config_path,
            Some(PathBuf::from(
                crate::new_session_history::BUILTIN_CONFIG_KEY
            ))
        );
    }

    #[test]
    fn history_defaults_remain_editable_and_non_explicit() {
        let mut form = NewSessionForm::default();
        let skipped = vec!["build".to_string(), "test".to_string()];
        form.apply_default_skips(&skipped);
        assert_eq!(form.selected_skipped_steps(), skipped);
        assert!(!form.skipped_explicit);
        form.step = Step::SkippedSteps;
        form.input(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        assert!(form.skipped_explicit);
        form.apply_default_skips(&["other".to_string()]);
        assert!(!form.selected_skipped_steps().contains(&"other".to_string()));
    }

    #[test]
    fn recent_working_directories_cycle_in_order() {
        let mut form = NewSessionForm::default();
        let recent = vec!["/tmp/one".to_string(), "/tmp/two".to_string()];
        form.cycle_working_directory(&recent, 1);
        assert_eq!(form.working_dir.text(), "/tmp/one");
        form.cycle_working_directory(&recent, 1);
        assert_eq!(form.working_dir.text(), "/tmp/two");
        form.cycle_working_directory(&recent, 1);
        assert_eq!(form.working_dir.text(), "/tmp/one");
    }

    #[test]
    fn request_maps_empty_working_directory_to_current_directory() {
        let form = NewSessionForm::default();
        assert_eq!(form.request().base_dir, PathBuf::from("."));
    }

    #[test]
    fn formal_spec_choice_can_be_enabled_and_disabled() {
        let mut form = NewSessionForm::default();
        form.step = Step::FormalSpec;

        form.choose(1);
        assert!(form.options.planning.formal_spec);
        form.choose(-1);
        assert!(!form.options.planning.formal_spec);
    }

    #[test]
    fn formal_spec_clears_input_plan_mode() {
        let mut form = NewSessionForm::default();
        form.options.planning.skip_planning = true;
        form.step = Step::FormalSpec;
        form.choose(1);
        assert!(form.options.planning.formal_spec);
        assert!(!form.options.planning.skip_planning);
    }

    #[test]
    fn validate_rejects_formal_spec_with_skip_planning_even_when_set_directly() {
        let mut form = NewSessionForm::default();
        form.options.planning.formal_spec = true;
        form.options.planning.skip_planning = true;

        assert!(form.validate().is_err());
    }

    #[test]
    fn directory_dialogue_asks_directory_questions_only() {
        let form = NewSessionForm::default();
        assert_eq!(
            form.steps().collect::<Vec<_>>(),
            vec![
                Step::Task,
                Step::Attachments,
                Step::Source,
                Step::WorkingDirectory,
                Step::Config,
                Step::SkippedSteps,
                Step::Workspace,
                Step::FormalSpec,
                Step::Launch,
            ]
        );
    }

    #[test]
    fn dirty_tree_question_appears_only_for_current_branch_runs() {
        let mut form = NewSessionForm::default();
        assert!(!form.applies(Step::DirtyTree));
        form.step = Step::Workspace;
        form.choose(1);
        assert_eq!(form.workspace_mode, WorkspaceMode::CurrentBranch);
        assert!(form.applies(Step::DirtyTree));
        assert!(form.advance());
        assert_eq!(form.step, Step::DirtyTree);
        assert!(form.advance());
        assert_eq!(form.step, Step::FormalSpec);
    }

    #[test]
    fn github_dialogue_replaces_directory_questions_with_repository() {
        let mut form = NewSessionForm {
            step: Step::Source,
            ..NewSessionForm::default()
        };
        form.workspace_mode = WorkspaceMode::CurrentBranch;
        form.choose(1);
        assert_eq!(form.source, SourceKind::GitHub);
        assert_eq!(
            form.workspace_mode,
            WorkspaceMode::Worktree,
            "GitHub clones always run in a worktree"
        );
        assert!(form.advance());
        assert_eq!(form.step, Step::Repository);
        assert!(form.advance());
        assert_eq!(form.step, Step::Config);
        assert!(form.advance());
        assert_eq!(form.step, Step::SkippedSteps);
        assert!(form.advance());
        assert_eq!(form.step, Step::FormalSpec);
        assert!(form.advance());
        assert_eq!(form.step, Step::Launch);
        assert!(!form.advance(), "Launch is the last question");
        assert_eq!(form.step, Step::Launch);
    }

    #[test]
    fn retreat_walks_back_to_the_first_question_and_stops() {
        let mut form = NewSessionForm::default();
        form.step = Step::Source;
        assert!(form.retreat());
        assert_eq!(form.step, Step::Attachments);
        assert!(form.retreat());
        assert_eq!(form.step, Step::Task);
        assert!(!form.retreat());
        assert_eq!(form.step, Step::Task);
    }

    #[test]
    fn launch_choice_cycles_and_maps_to_planning_flags() {
        let mut form = NewSessionForm::default();
        form.step = Step::Launch;
        form.choose(-1);
        assert_eq!(form.launch, Launch::SaveDraft);
        form.choose(1);
        assert_eq!(form.launch, Launch::Planning);
        form.choose(2);
        assert_eq!(form.launch, Launch::InputPlan);

        form.options.planning.formal_spec = true;
        form.select_launch(Launch::InputPlan);
        assert!(form.options.planning.skip_planning);
        assert!(!form.options.planning.grill);
        assert!(
            !form.options.planning.formal_spec,
            "input-as-plan cannot carry a formal specification"
        );
        form.select_launch(Launch::Grill);
        assert!(form.options.planning.grill);
        assert!(!form.options.planning.skip_planning);
        assert!(form.validate().is_ok());
    }

    #[test]
    fn step_validation_requires_task_or_image_and_repository() {
        let mut form = NewSessionForm::default();
        form.step = Step::Attachments;
        assert_eq!(
            form.validate_step(),
            Err("Task description or an image attachment is required".to_string())
        );
        form.attachments.set_text("shot.png");
        assert!(form.validate_step().is_ok());
        form.attachments.set_text("");
        form.input.set_text("do the thing");
        assert!(form.validate_step().is_ok());

        form.source = SourceKind::GitHub;
        form.step = Step::Repository;
        assert_eq!(
            form.validate_step(),
            Err("Select a GitHub repository before creating a session".to_string())
        );
        form.repository.set_text("acme/cruise");
        assert!(form.validate_step().is_ok());
    }

    #[test]
    fn answers_summarise_each_step() {
        let mut form = NewSessionForm::default();
        assert_eq!(form.answer(Step::Task), "(none)");
        form.input.set_text("first line\nsecond line");
        assert_eq!(form.answer(Step::Task), "first line …");
        assert_eq!(form.answer(Step::Attachments), "none");
        form.attachments.set_text("a.png\nb.png");
        assert_eq!(form.answer(Step::Attachments), "2 images");
        assert_eq!(form.answer(Step::WorkingDirectory), "current directory");
        assert_eq!(form.answer(Step::Config), "auto-detect");
        assert_eq!(form.answer(Step::SkippedSteps), "none");
        form.skipped.set_text("build, test");
        assert_eq!(form.answer(Step::SkippedSteps), "build, test");
        assert_eq!(form.answer(Step::Workspace), "worktree");
        assert_eq!(form.answer(Step::FormalSpec), "no");
        assert_eq!(form.answer(Step::Launch), "Start planning");
    }
}
