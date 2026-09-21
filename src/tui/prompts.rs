use crate::application::{OptionChoicePayload, PendingPrompt, PendingPromptKind};
use crate::step::option::OptionResult;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::HashSet;

use super::forms::Editor;

#[derive(Debug, Clone)]
pub struct PromptItem {
    pub request_id: String,
    pub session_id: String,
    pub question: String,
    pub choices: Vec<OptionChoicePayload>,
}

impl From<PendingPrompt> for PromptItem {
    fn from(prompt: PendingPrompt) -> Self {
        Self {
            request_id: prompt.request_id,
            session_id: prompt.session_id,
            question: prompt
                .question
                .unwrap_or_else(|| "Choose an option".to_string()),
            choices: prompt.choices,
        }
    }
}

#[derive(Default)]
pub struct PromptQueue {
    items: Vec<PromptItem>,
    pub active: Option<PromptItem>,
    pub answer: Editor,
    pub choice: usize,
}

impl PromptQueue {
    pub fn enqueue(&mut self, prompt: PromptItem) {
        if !self
            .items
            .iter()
            .any(|old| old.request_id == prompt.request_id)
            && self
                .active
                .as_ref()
                .is_none_or(|old| old.request_id != prompt.request_id)
        {
            self.items.push(prompt);
        }
    }

    pub fn sync_session(
        &mut self,
        session_id: &str,
        prompts: impl IntoIterator<Item = PendingPrompt>,
    ) {
        let prompts = prompts
            .into_iter()
            .map(Into::into)
            .collect::<Vec<PromptItem>>();
        let valid = prompts
            .iter()
            .map(|prompt| prompt.request_id.clone())
            .collect::<std::collections::HashSet<_>>();
        self.items
            .retain(|item| item.session_id != session_id || valid.contains(&item.request_id));
        if self
            .active
            .as_ref()
            .is_some_and(|item| item.session_id == session_id && !valid.contains(&item.request_id))
        {
            self.active = None;
            self.answer = Editor::default();
            self.choice = 0;
        }
        for prompt in prompts {
            self.enqueue(prompt);
        }
    }
    pub fn requeue_active(&mut self) {
        if let Some(item) = self.active.take()
            && !self
                .items
                .iter()
                .any(|queued| queued.request_id == item.request_id)
        {
            self.items.insert(0, item);
        }
        self.answer = Editor::default();
        self.choice = 0;
    }
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len() + usize::from(self.active.is_some())
    }
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn open_next(&mut self) {
        if self.active.is_some() || self.items.is_empty() {
            return;
        }
        let item = self.items.remove(0);
        self.choice = 0;
        self.answer = Editor::default();
        self.active = Some(item);
    }
    pub fn retain_sessions(&mut self, sessions: &std::collections::HashSet<String>) {
        self.items
            .retain(|item| sessions.contains(&item.session_id));
        if self
            .active
            .as_ref()
            .is_some_and(|item| !sessions.contains(&item.session_id))
        {
            self.active = None;
            self.answer = Editor::default();
            self.choice = 0;
        }
    }
    pub fn close_active(&mut self) {
        self.active = None;
    }
    pub fn move_choice(&mut self, delta: isize) {
        let Some(item) = self.active.as_ref() else {
            return;
        };
        if item.choices.is_empty() {
            return;
        }
        let len = item.choices.len();
        let choice = self.choice % len;
        let offset = delta.unsigned_abs() % len;
        self.choice = if delta.is_negative() {
            if choice >= offset {
                choice - offset
            } else {
                len - (offset - choice)
            }
        } else if choice >= len - offset {
            choice - (len - offset)
        } else {
            choice + offset
        };
    }
    pub fn selected_option(&self) -> Option<OptionResult> {
        let item = self.active.as_ref()?;
        let choice = item.choices.get(self.choice)?;
        let text_input = matches!(
            &choice.kind,
            &crate::application::OptionChoiceKind::TextInput
        )
        .then(|| self.answer.text());
        if text_input
            .as_deref()
            .is_some_and(|text| text.trim().is_empty())
        {
            return None;
        }
        Some(OptionResult {
            next_step: choice.next_step.clone(),
            text_input,
        })
    }
}

pub struct PlanPromptItem {
    pub request_id: String,
    pub session_id: String,
    pub question: String,
    pub answer: Editor,
    pub question_scroll: usize,
    pub error: Option<String>,
    pub editing: bool,
}

impl PlanPromptItem {
    fn new(session_id: String, request_id: String, question: String) -> Self {
        Self {
            request_id,
            session_id,
            question,
            answer: Editor::default(),
            question_scroll: 0,
            error: None,
            editing: false,
        }
    }
}

#[derive(Default)]
pub struct PlanPromptStore {
    items: Vec<PlanPromptItem>,
}

impl PlanPromptStore {
    pub fn enqueue(&mut self, session_id: String, request_id: String, question: String) {
        if let Some(item) = self
            .items
            .iter_mut()
            .find(|item| item.session_id == session_id && item.request_id == request_id)
        {
            item.question = question;
            return;
        }
        self.items
            .push(PlanPromptItem::new(session_id, request_id, question));
    }

    pub fn sync_session(
        &mut self,
        session_id: &str,
        prompts: impl IntoIterator<Item = PendingPrompt>,
    ) {
        let prompts = prompts
            .into_iter()
            .filter(|prompt| matches!(&prompt.kind, PendingPromptKind::Ask))
            .collect::<Vec<_>>();
        let valid = prompts
            .iter()
            .map(|prompt| prompt.request_id.as_str())
            .collect::<HashSet<_>>();
        self.items.retain(|item| {
            item.session_id != session_id || valid.contains(item.request_id.as_str())
        });

        for prompt in prompts {
            let question = prompt.question.unwrap_or_default();
            if let Some(item) = self
                .items
                .iter_mut()
                .find(|item| item.session_id == session_id && item.request_id == prompt.request_id)
            {
                if !question.is_empty() {
                    item.question = question;
                }
            } else {
                self.enqueue(session_id.to_string(), prompt.request_id, question);
            }
        }
    }

    pub fn retain_sessions(&mut self, sessions: &HashSet<String>) {
        self.items
            .retain(|item| sessions.contains(&item.session_id));
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn has_session(&self, session_id: &str) -> bool {
        self.items.iter().any(|item| item.session_id == session_id)
    }

    #[must_use]
    pub fn for_session(&self, session_id: &str) -> Option<&PlanPromptItem> {
        self.items.iter().find(|item| item.session_id == session_id)
    }

    fn for_session_mut(&mut self, session_id: &str) -> Option<&mut PlanPromptItem> {
        self.items
            .iter_mut()
            .find(|item| item.session_id == session_id)
    }

    pub fn remove(&mut self, session_id: &str, request_id: &str) {
        self.items
            .retain(|item| item.session_id != session_id || item.request_id != request_id);
    }

    pub fn begin_editing(&mut self, session_id: &str) {
        if let Some(item) = self.for_session_mut(session_id) {
            item.editing = true;
            item.error = None;
        }
    }

    pub fn end_editing(&mut self, session_id: &str) {
        if let Some(item) = self.for_session_mut(session_id) {
            item.editing = false;
        }
    }

    pub fn clear_focus(&mut self) {
        for item in &mut self.items {
            item.editing = false;
        }
    }

    pub fn input(&mut self, session_id: &str, key: KeyEvent) {
        let Some(item) = self.for_session_mut(session_id) else {
            return;
        };
        item.answer.input(key);
        item.error = None;
    }

    #[must_use]
    pub fn answer_text(&self, session_id: &str) -> Option<String> {
        let answer = self.for_session(session_id)?.answer.text();
        (!answer.trim().is_empty()).then_some(answer)
    }

    pub fn set_error(&mut self, session_id: &str, error: String) {
        if let Some(item) = self.for_session_mut(session_id) {
            item.error = Some(error);
            item.editing = true;
        }
    }

    pub fn scroll(&mut self, session_id: &str, delta: isize) {
        let Some(item) = self.for_session_mut(session_id) else {
            return;
        };
        let max_scroll = item.question.lines().count().saturating_sub(1);
        item.question_scroll = if delta.is_negative() {
            item.question_scroll.saturating_sub(delta.unsigned_abs())
        } else {
            item.question_scroll.saturating_add(delta.cast_unsigned())
        }
        .min(max_scroll);
    }

    pub fn scroll_home(&mut self, session_id: &str) {
        if let Some(item) = self.for_session_mut(session_id) {
            item.question_scroll = 0;
        }
    }

    pub fn scroll_end(&mut self, session_id: &str) {
        if let Some(item) = self.for_session_mut(session_id) {
            item.question_scroll = item.question.lines().count().saturating_sub(1);
        }
    }

    #[must_use]
    pub fn is_editing(&self, session_id: &str) -> bool {
        self.for_session(session_id)
            .is_some_and(|item| item.editing)
    }

    #[must_use]
    pub fn request_for_session(&self, session_id: &str) -> Option<(String, String)> {
        self.for_session(session_id)
            .map(|item| (item.session_id.clone(), item.request_id.clone()))
    }

    #[must_use]
    pub fn is_editor_key(key: KeyEvent) -> bool {
        !key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(
                key.code,
                KeyCode::Char(_)
                    | KeyCode::Backspace
                    | KeyCode::Left
                    | KeyCode::Right
                    | KeyCode::Home
                    | KeyCode::End
                    | KeyCode::Up
                    | KeyCode::Down
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn option(session_id: &str, request_id: &str) -> PendingPrompt {
        PendingPrompt {
            request_id: request_id.to_string(),
            session_id: session_id.to_string(),
            kind: PendingPromptKind::Option,
            question: Some("Choose a target".to_string()),
            choices: vec![OptionChoicePayload {
                label: "staging".to_string(),
                kind: crate::application::OptionChoiceKind::Selector,
                next_step: None,
            }],
        }
    }

    fn text_option(request_id: &str) -> PendingPrompt {
        PendingPrompt {
            request_id: request_id.to_string(),
            session_id: "s".to_string(),
            kind: PendingPromptKind::Option,
            question: Some("Choose a target".to_string()),
            choices: vec![OptionChoicePayload {
                label: "Other".to_string(),
                kind: crate::application::OptionChoiceKind::TextInput,
                next_step: None,
            }],
        }
    }

    fn ask(session_id: &str, request_id: &str, question: &str) -> PendingPrompt {
        PendingPrompt {
            request_id: request_id.to_string(),
            session_id: session_id.to_string(),
            kind: PendingPromptKind::Ask,
            question: Some(question.to_string()),
            choices: Vec::new(),
        }
    }

    #[test]
    fn option_queue_deduplicates_and_opens_fifo() {
        let p = option("s", "one");
        let mut queue = PromptQueue::default();
        queue.enqueue(p.clone().into());
        queue.enqueue(p.into());
        assert_eq!(queue.len(), 1);
        queue.open_next();
        assert_eq!(
            queue.active.as_ref().map(|p| p.request_id.as_str()),
            Some("one")
        );
    }

    #[test]
    fn empty_text_option_is_rejected_and_active_prompt_is_retained() {
        let mut queue = PromptQueue::default();
        queue.enqueue(text_option("one").into());
        queue.open_next();
        assert!(queue.selected_option().is_none());
        queue.requeue_active();
        assert!(queue.active.is_none());
        assert_eq!(queue.len(), 1);
    }

    #[test]
    fn empty_text_option_is_rejected() {
        let mut queue = PromptQueue::default();
        queue.enqueue(text_option("option").into());
        queue.open_next();
        assert!(queue.selected_option().is_none());
    }

    #[test]
    fn syncing_one_session_does_not_remove_other_session_options() {
        let mut queue = PromptQueue::default();
        queue.enqueue(option("session-a", "option-a").into());
        queue.enqueue(option("session-b", "option-b").into());

        queue.sync_session("session-a", [option("session-a", "option-a-new")]);

        assert_eq!(queue.len(), 2);
        let mut request_ids = Vec::new();
        queue.open_next();
        request_ids.push(queue.active.as_ref().map_or_else(
            || panic!("missing first option"),
            |prompt| prompt.request_id.clone(),
        ));
        queue.close_active();
        queue.open_next();
        request_ids.push(queue.active.as_ref().map_or_else(
            || panic!("missing second option"),
            |prompt| prompt.request_id.clone(),
        ));
        request_ids.sort();
        assert_eq!(request_ids, ["option-a-new", "option-b"]);
    }

    #[test]
    fn syncing_an_active_request_preserves_its_answer_draft() {
        let mut queue = PromptQueue::default();
        queue.enqueue(option("session-a", "option-a").into());
        queue.open_next();
        queue.answer.set_text("keep this draft");

        queue.sync_session("session-a", [option("session-a", "option-a")]);

        assert_eq!(queue.answer.text(), "keep this draft");
        assert_eq!(
            queue
                .active
                .as_ref()
                .map(|prompt| prompt.request_id.as_str()),
            Some("option-a")
        );
    }

    #[test]
    fn duplicate_plan_request_updates_question_without_resetting_draft_or_editing() {
        let mut store = PlanPromptStore::default();
        store.enqueue(
            "session".to_string(),
            "ask-1".to_string(),
            "First question".to_string(),
        );
        store.begin_editing("session");
        store.input(
            "session",
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
        );

        store.enqueue(
            "session".to_string(),
            "ask-1".to_string(),
            "Updated question".to_string(),
        );

        let prompt = store
            .for_session("session")
            .unwrap_or_else(|| panic!("missing plan prompt"));
        assert_eq!(prompt.question, "Updated question");
        assert_eq!(prompt.answer.text(), "d");
        assert!(prompt.editing);
    }

    #[test]
    fn syncing_session_removes_invalid_plan_requests() {
        let mut store = PlanPromptStore::default();
        store.enqueue(
            "session".to_string(),
            "ask-old".to_string(),
            "Old".to_string(),
        );
        store.enqueue(
            "session".to_string(),
            "ask-live".to_string(),
            "Live".to_string(),
        );

        store.sync_session("session", [ask("session", "ask-live", "Updated live")]);

        assert_eq!(store.len(), 1);
        assert_eq!(
            store
                .for_session("session")
                .map(|prompt| prompt.request_id.as_str()),
            Some("ask-live")
        );
    }

    #[test]
    fn syncing_one_session_retains_prompts_from_other_sessions() {
        let mut store = PlanPromptStore::default();
        store.enqueue(
            "session-a".to_string(),
            "ask-a".to_string(),
            "A".to_string(),
        );
        store.enqueue(
            "session-b".to_string(),
            "ask-b".to_string(),
            "B".to_string(),
        );

        store.sync_session("session-a", std::iter::empty::<PendingPrompt>());

        assert_eq!(store.len(), 1);
        assert!(!store.has_session("session-a"));
        assert!(store.has_session("session-b"));
    }

    #[test]
    fn retaining_sessions_cleans_up_deleted_plan_prompts() {
        let mut store = PlanPromptStore::default();
        store.enqueue(
            "deleted".to_string(),
            "ask-deleted".to_string(),
            "Deleted".to_string(),
        );
        store.enqueue(
            "live".to_string(),
            "ask-live".to_string(),
            "Live".to_string(),
        );

        store.retain_sessions(&HashSet::from(["live".to_string()]));

        assert_eq!(store.len(), 1);
        assert!(!store.has_session("deleted"));
        assert!(store.has_session("live"));
    }

    #[test]
    fn syncing_one_session_preserves_another_sessions_draft_and_editing() {
        let mut store = PlanPromptStore::default();
        store.enqueue(
            "session-a".to_string(),
            "ask-a".to_string(),
            "A".to_string(),
        );
        store.enqueue(
            "session-b".to_string(),
            "ask-b".to_string(),
            "B".to_string(),
        );
        store.begin_editing("session-b");
        store.input(
            "session-b",
            KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE),
        );

        store.sync_session("session-a", [ask("session-a", "ask-a", "Updated A")]);

        assert_eq!(store.answer_text("session-b").as_deref(), Some("k"));
        assert!(store.is_editing("session-b"));
        assert_eq!(
            store
                .for_session("session-a")
                .map(|prompt| prompt.question.as_str()),
            Some("Updated A")
        );
    }
}
