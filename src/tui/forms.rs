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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    Directory,
    GitHub,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormField {
    Source,
    Input,
    WorkingDirectory,
    Repository,
    Config,
    Attachments,
    SkippedSteps,
    Workspace,
    DirtyTree,
    Grill,
    FormalSpec,
    SkipPlanning,
    Noninteractive,
    SaveDraft,
    Submit,
}

impl FormField {
    #[must_use]
    pub fn next(self) -> Self {
        match self {
            Self::Source => Self::Input,
            Self::Input => Self::WorkingDirectory,
            Self::WorkingDirectory => Self::Repository,
            Self::Repository => Self::Config,
            Self::Config => Self::Attachments,
            Self::Attachments => Self::SkippedSteps,
            Self::SkippedSteps => Self::Workspace,
            Self::Workspace => Self::DirtyTree,
            Self::DirtyTree => Self::Grill,
            Self::Grill => Self::FormalSpec,
            Self::FormalSpec => Self::SkipPlanning,
            Self::SkipPlanning => Self::Noninteractive,
            Self::Noninteractive => Self::SaveDraft,
            Self::SaveDraft => Self::Submit,
            Self::Submit => Self::Source,
        }
    }
    #[must_use]
    pub fn previous(self) -> Self {
        match self {
            Self::Source => Self::Submit,
            Self::Input => Self::Source,
            Self::WorkingDirectory => Self::Input,
            Self::Repository => Self::WorkingDirectory,
            Self::Config => Self::Repository,
            Self::Attachments => Self::Config,
            Self::SkippedSteps => Self::Attachments,
            Self::Workspace => Self::SkippedSteps,
            Self::DirtyTree => Self::Workspace,
            Self::Grill => Self::DirtyTree,
            Self::FormalSpec => Self::Grill,
            Self::SkipPlanning => Self::FormalSpec,
            Self::Noninteractive => Self::SkipPlanning,
            Self::SaveDraft => Self::Noninteractive,
            Self::Submit => Self::SaveDraft,
        }
    }
}

/// State for the New Session screen.  Only the draft fields persist; focus and
/// provider toggles are intentionally ephemeral.
pub struct NewSessionForm {
    pub input: Editor,
    pub working_dir: Editor,
    pub repository: Editor,
    pub config: Editor,
    pub attachments: Editor,
    pub skipped: Editor,
    pub field: FormField,
    pub editing: bool,
    pub source: SourceKind,
    pub workspace_mode: WorkspaceMode,
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
#[expect(
    clippy::struct_excessive_bools,
    reason = "the TUI exposes independent planning toggles, including formal specification mode"
)]
pub struct PlanningFlags {
    pub grill: bool,
    pub formal_spec: bool,
    pub skip_planning: bool,
    pub noninteractive: bool,
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
            field: FormField::Input,
            editing: false,
            source: if draft.repo.is_some() {
                SourceKind::GitHub
            } else {
                SourceKind::Directory
            },
            workspace_mode: WorkspaceMode::Worktree,
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
        let attachments = self
            .attachments
            .text()
            .lines()
            .filter_map(|line| {
                let path = line.trim();
                (!path.is_empty())
                    .then(|| PathBuf::from(crate::new_session_history::expand_tilde(path)))
            })
            .collect();
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
            attachments,
            skipped_steps: self.selected_skipped_steps(),
        }
    }

    pub fn toggle_workspace(&mut self) {
        self.workspace_mode = match self.workspace_mode {
            WorkspaceMode::Worktree => WorkspaceMode::CurrentBranch,
            WorkspaceMode::CurrentBranch => WorkspaceMode::Worktree,
        };
        self.mark_changed();
    }
    pub fn toggle_current(&mut self) {
        match self.field {
            FormField::Source => {
                self.source = match self.source {
                    SourceKind::Directory => SourceKind::GitHub,
                    SourceKind::GitHub => SourceKind::Directory,
                };
                if self.source == SourceKind::GitHub {
                    self.workspace_mode = WorkspaceMode::Worktree;
                }
                self.mark_changed();
            }
            FormField::Workspace => self.toggle_workspace(),
            FormField::DirtyTree => {
                self.options.allow_dirty_working_tree = !self.options.allow_dirty_working_tree;
                self.mark_changed();
            }
            FormField::Grill => {
                self.options.planning.grill = !self.options.planning.grill;
                if self.options.planning.grill {
                    self.options.planning.skip_planning = false;
                    self.options.planning.noninteractive = false;
                }
                self.mark_changed();
            }
            FormField::FormalSpec => {
                self.options.planning.formal_spec = !self.options.planning.formal_spec;
                if self.options.planning.formal_spec {
                    self.options.planning.skip_planning = false;
                }
                self.mark_changed();
            }
            FormField::SkipPlanning => {
                self.options.planning.skip_planning = !self.options.planning.skip_planning;
                if self.options.planning.skip_planning {
                    self.options.planning.grill = false;
                    self.options.planning.formal_spec = false;
                    self.options.planning.noninteractive = false;
                }
                self.mark_changed();
            }
            FormField::Noninteractive => {
                self.options.planning.noninteractive = !self.options.planning.noninteractive;
                if self.options.planning.noninteractive {
                    self.options.planning.grill = false;
                    self.options.planning.skip_planning = false;
                }
                self.mark_changed();
            }
            _ => {}
        }
    }
    pub fn input(&mut self, event: KeyEvent) {
        match self.field {
            FormField::Input => self.input.input(event),
            FormField::WorkingDirectory => self.working_dir.input(event),
            FormField::Repository => self.repository.input(event),
            FormField::Config => self.config.input(event),
            FormField::Attachments => self.attachments.input(event),
            FormField::SkippedSteps => self.skipped.input(event),
            _ => return,
        }
        self.mark_changed();
        if self.field == FormField::SkippedSteps {
            self.skipped_explicit = true;
        }
    }

    pub fn set_field(&mut self, field: FormField) {
        self.field = field;
        self.editing = false;
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

    /// Cycle through recent working directories recorded by session history.
    pub fn cycle_working_directory(&mut self, paths: &[String]) {
        if paths.is_empty() {
            return;
        }
        let current = self.working_dir.text();
        let index = paths
            .iter()
            .position(|path| path == current.trim())
            .map_or(0, |index| (index + 1) % paths.len());
        self.working_dir.set_text(&paths[index]);
        self.mark_changed();
    }

    pub fn cycle_config(&mut self, sources: &[crate::configs::ConfigEntry]) {
        if sources.is_empty() {
            return;
        }
        let config = self.config.text();
        let current = config.trim();
        let index = sources
            .iter()
            .position(|entry| entry.path == current)
            .map_or(0, |idx| (idx + 1) % sources.len());
        self.config.set_text(&sources[index].path);
        self.mark_changed();
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.options.planning.grill
            && (self.options.planning.skip_planning || self.options.planning.noninteractive)
        {
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

#[cfg(test)]
#[expect(
    clippy::field_reassign_with_default,
    reason = "form tests intentionally switch focus and toggle individual controls"
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
            field: FormField::Source,
            ..NewSessionForm::default()
        };
        form.toggle_current();
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
    fn config_cycles_and_text_input_marks_form_dirty() {
        let mut form = NewSessionForm::default();
        let sources = [
            crate::configs::ConfigEntry {
                name: "A".to_string(),
                path: "a.yaml".to_string(),
                description: None,
            },
            crate::configs::ConfigEntry {
                name: "B".to_string(),
                path: "b.yaml".to_string(),
                description: None,
            },
        ];
        form.cycle_config(&sources);
        assert_eq!(form.config.text(), "a.yaml");
        form.cycle_config(&sources);
        assert_eq!(form.config.text(), "b.yaml");

        form.set_field(FormField::SkippedSteps);
        form.input(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        assert!(form.dirty);
        assert!(form.skipped_explicit);
    }
    #[test]
    fn history_defaults_remain_editable_and_non_explicit() {
        let mut form = NewSessionForm::default();
        let skipped = vec!["build".to_string(), "test".to_string()];
        form.apply_default_skips(&skipped);
        assert_eq!(form.selected_skipped_steps(), skipped);
        assert!(!form.skipped_explicit);
        form.set_field(FormField::SkippedSteps);
        form.input(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        assert!(form.skipped_explicit);
        form.apply_default_skips(&["other".to_string()]);
        assert!(!form.selected_skipped_steps().contains(&"other".to_string()));
    }

    #[test]
    fn recent_working_directories_cycle_in_order() {
        let mut form = NewSessionForm::default();
        let recent = vec!["/tmp/one".to_string(), "/tmp/two".to_string()];
        form.cycle_working_directory(&recent);
        assert_eq!(form.working_dir.text(), "/tmp/one");
        form.cycle_working_directory(&recent);
        assert_eq!(form.working_dir.text(), "/tmp/two");
    }

    #[test]
    fn request_maps_empty_working_directory_to_current_directory() {
        let form = NewSessionForm::default();
        assert_eq!(form.request().base_dir, PathBuf::from("."));
    }

    #[test]
    fn formal_spec_toggle_can_be_enabled_and_disabled() {
        let mut form = NewSessionForm::default();
        form.field = FormField::FormalSpec;

        form.toggle_current();
        assert!(form.options.planning.formal_spec);
        form.toggle_current();
        assert!(!form.options.planning.formal_spec);
    }

    #[test]
    fn formal_spec_and_skip_planning_are_mutually_exclusive() {
        let mut form = NewSessionForm::default();
        form.field = FormField::SkipPlanning;
        form.toggle_current();
        assert!(form.options.planning.skip_planning);

        form.field = FormField::FormalSpec;
        form.toggle_current();
        assert!(form.options.planning.formal_spec);
        assert!(!form.options.planning.skip_planning);

        form.field = FormField::SkipPlanning;
        form.toggle_current();
        assert!(!form.options.planning.formal_spec);
        assert!(form.options.planning.skip_planning);
    }

    #[test]
    fn formal_spec_can_coexist_with_grill_and_noninteractive_planning() {
        let mut form = NewSessionForm::default();
        form.field = FormField::FormalSpec;
        form.toggle_current();

        form.field = FormField::Grill;
        form.toggle_current();
        assert!(form.options.planning.formal_spec);
        assert!(form.options.planning.grill);

        form.field = FormField::Grill;
        form.toggle_current();
        form.field = FormField::Noninteractive;
        form.toggle_current();
        assert!(form.options.planning.formal_spec);
        assert!(form.options.planning.noninteractive);
    }

    #[test]
    fn validate_rejects_formal_spec_with_skip_planning_even_when_set_directly() {
        let mut form = NewSessionForm::default();
        form.options.planning.formal_spec = true;
        form.options.planning.skip_planning = true;

        assert!(form.validate().is_err());
    }

    #[test]
    fn formal_spec_is_reachable_in_both_focus_directions() {
        let mut next = FormField::Source;
        let mut reached_forward = false;
        for _ in 0..32 {
            if next == FormField::FormalSpec {
                reached_forward = true;
                break;
            }
            next = next.next();
        }
        assert!(
            reached_forward,
            "Tab traversal must include Formal specification"
        );

        let mut previous = FormField::Submit;
        let mut reached_backward = false;
        for _ in 0..32 {
            if previous == FormField::FormalSpec {
                reached_backward = true;
                break;
            }
            previous = previous.previous();
        }
        assert!(
            reached_backward,
            "reverse Tab traversal must include Formal specification"
        );
    }
}
