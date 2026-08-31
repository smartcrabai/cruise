use crate::application::{OptionChoicePayload, PendingPrompt, PendingPromptKind};
use crate::step::option::OptionResult;

use super::forms::Editor;

#[derive(Debug, Clone)]
pub struct PromptItem {
    pub request_id: String,
    pub session_id: String,
    pub kind: PendingPromptKind,
    pub question: String,
    pub choices: Vec<OptionChoicePayload>,
}

impl From<PendingPrompt> for PromptItem {
    fn from(prompt: PendingPrompt) -> Self {
        Self {
            request_id: prompt.request_id,
            session_id: prompt.session_id,
            kind: prompt.kind,
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
    pub fn answer_text(&self) -> Option<String> {
        let answer = self.answer.text();
        (!answer.trim().is_empty()).then_some(answer)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn ask(request_id: &str) -> PendingPrompt {
        PendingPrompt {
            request_id: request_id.to_string(),
            session_id: "s".to_string(),
            kind: PendingPromptKind::Ask,
            question: Some("What next?".to_string()),
            choices: vec![],
        }
    }

    #[test]
    fn queue_deduplicates_and_opens_fifo() {
        let p = ask("one");
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
    fn empty_free_text_is_rejected_and_active_prompt_is_retained() {
        let mut queue = PromptQueue::default();
        queue.enqueue(ask("one").into());
        queue.open_next();
        assert!(queue.answer_text().is_none());
        queue.requeue_active();
        assert!(queue.active.is_none());
        assert_eq!(queue.len(), 1);
    }

    #[test]
    fn empty_text_option_is_rejected() {
        let prompt = PendingPrompt {
            request_id: "option".to_string(),
            session_id: "s".to_string(),
            kind: PendingPromptKind::Option,
            question: Some("Choose".to_string()),
            choices: vec![OptionChoicePayload {
                label: "Other".to_string(),
                kind: crate::application::OptionChoiceKind::TextInput,
                next_step: None,
            }],
        };
        let mut queue = PromptQueue::default();
        queue.enqueue(prompt.into());
        queue.open_next();
        assert!(queue.selected_option().is_none());
    }
}
