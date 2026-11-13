use std::{collections::VecDeque, time::Instant};
use telinha_shared::feedback::{FeedbackRequest, MAX_FEEDBACK_COMMENT_CHARS};

// Receipts exist only in memory. Deliberately do not implement Debug.
pub struct Receipt {
    pub token: String,
    pub connected_at: Instant,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    Left,
    Switched,
    Disconnected,
    Sharing,
}
impl Reason {
    fn as_str(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Switched => "switched",
            Self::Disconnected => "disconnected",
            Self::Sharing => "sharing",
        }
    }
}
struct Entry {
    generation: u64,
    receipt: Receipt,
    label: String,
    sharing_prompt_dismissed: bool,
}
struct Attempt {
    generation: u64,
    label: String,
}
struct EndedAttempt {
    attempt: Attempt,
    reason: Reason,
    ended_at: Instant,
}
#[derive(Default)]
pub struct State {
    active: Option<Entry>,
    connecting: Option<Attempt>,
    ended_attempts: VecDeque<EndedAttempt>,
    last_generation: Option<u64>,
    pending: VecDeque<Draft>,
}
impl State {
    pub fn starting(&mut self, generation: u64, label: String) {
        self.connecting = Some(Attempt { generation, label });
    }
    pub fn late_connected(&mut self, generation: u64, receipt: Option<Receipt>) {
        let Some(index) = self
            .ended_attempts
            .iter()
            .position(|entry| entry.attempt.generation == generation)
        else {
            return;
        };
        let ended = self.ended_attempts.remove(index).expect("matching attempt");
        if let Some(receipt) = receipt
            .filter(|receipt| !receipt.token.is_empty() && receipt.connected_at <= ended.ended_at)
        {
            self.enqueue(
                Entry {
                    generation,
                    receipt,
                    label: ended.attempt.label,
                    sharing_prompt_dismissed: false,
                },
                ended.reason,
                ended.ended_at,
            );
        }
    }
    pub fn connected(&mut self, generation: u64, receipt: Option<Receipt>, label: String) {
        if self.last_generation.is_some_and(|last| generation <= last) {
            return;
        }
        self.last_generation = Some(generation);
        let label = if self
            .connecting
            .as_ref()
            .is_some_and(|attempt| attempt.generation == generation)
        {
            self.connecting.take().expect("matching attempt").label
        } else {
            label
        };
        self.active = receipt
            .filter(|receipt| !receipt.token.is_empty())
            .map(|receipt| Entry {
                generation,
                receipt,
                label,
                sharing_prompt_dismissed: false,
            });
    }
    pub fn can_prompt_sharing(&self, generation: u64) -> bool {
        self.pending.is_empty()
            && self.active.as_ref().is_some_and(|entry| {
                entry.generation == generation && !entry.sharing_prompt_dismissed
            })
    }
    pub fn dismiss_sharing_prompt(&mut self, generation: u64) {
        if let Some(entry) = self
            .active
            .as_mut()
            .filter(|entry| entry.generation == generation)
        {
            entry.sharing_prompt_dismissed = true;
        }
    }
    pub fn request_sharing(&mut self, generation: u64, now: Instant) -> bool {
        if !self.can_prompt_sharing(generation) {
            return false;
        }
        let entry = self.active.take().expect("matching active receipt");
        self.enqueue(entry, Reason::Sharing, now);
        true
    }
    pub fn end(&mut self, generation: u64, reason: Reason, now: Instant) {
        if !self
            .active
            .as_ref()
            .is_some_and(|entry| entry.generation == generation)
        {
            if self
                .connecting
                .as_ref()
                .is_some_and(|attempt| attempt.generation == generation)
            {
                if self.ended_attempts.len() == 4 {
                    self.ended_attempts.pop_front();
                }
                self.ended_attempts.push_back(EndedAttempt {
                    attempt: self.connecting.take().expect("matching attempt"),
                    reason,
                    ended_at: now,
                });
            }
            return;
        }
        let entry = self.active.take().expect("matching entry");
        self.enqueue(entry, reason, now);
    }
    fn enqueue(&mut self, entry: Entry, reason: Reason, now: Instant) {
        // Keep a bounded queue without replacing a form already being answered.
        if self.pending.len() == 4 {
            return;
        }
        self.pending.push_back(Draft {
            id: entry.generation,
            label: entry.label,
            token: entry.receipt.token,
            duration_seconds: now
                .saturating_duration_since(entry.receipt.connected_at)
                .as_secs(),
            reason,
            audio_rating: None,
            video_rating: None,
            comment: String::new(),
            status: Status::Editing,
        });
    }
    pub fn pop_pending(&mut self) -> Option<Draft> {
        self.pending.pop_front()
    }
}
#[derive(PartialEq, Eq)]
pub enum Status {
    Editing,
    Sending,
    Failed(String),
    Sent,
}
pub struct Draft {
    pub id: u64,
    pub label: String,
    token: String,
    duration_seconds: u64,
    reason: Reason,
    pub audio_rating: Option<u8>,
    pub video_rating: Option<u8>,
    comment: String,
    pub status: Status,
}
impl Draft {
    pub fn during_sharing(&self) -> bool {
        self.reason == Reason::Sharing
    }
    pub fn editable(&self) -> bool {
        self.status == Status::Editing
    }
    pub fn can_send(&self, comment: &str) -> bool {
        match self.status {
            Status::Sending | Status::Sent => false,
            Status::Failed(_) => true,
            Status::Editing => valid_answer(self.audio_rating, self.video_rating, comment),
        }
    }
    pub fn submit(&mut self, comment: &str) -> Option<FeedbackRequest> {
        if !self.can_send(comment) {
            return None;
        }
        // A timeout may mean the server saved the first request. Retry it unchanged.
        if self.editable() {
            self.comment = comment.trim().to_owned();
        }
        self.status = Status::Sending;
        Some(FeedbackRequest {
            token: self.token.clone(),
            audio_rating: self.audio_rating,
            video_rating: self.video_rating,
            comment: self.comment.clone(),
            client: if cfg!(target_os = "windows") {
                "windows"
            } else {
                "macos"
            }
            .into(),
            app_version: env!("CARGO_PKG_VERSION").into(),
            duration_seconds: self.duration_seconds,
            reason: self.reason.as_str().into(),
        })
    }
    pub fn completed(&mut self, result: Result<(), String>) {
        if self.status != Status::Sending {
            return;
        }
        self.status = match result {
            Ok(()) => {
                self.token.clear();
                Status::Sent
            }
            Err(message) => Status::Failed(message),
        };
    }
}
fn valid_answer(audio: Option<u8>, video: Option<u8>, comment: &str) -> bool {
    [audio, video]
        .into_iter()
        .flatten()
        .all(|rating| (1..=5).contains(&rating))
        && comment.chars().count() <= MAX_FEEDBACK_COMMENT_CHARS
        && !comment
            .chars()
            .any(|ch| ch.is_control() && !matches!(ch, '\n' | '\r' | '\t'))
        && (audio.is_some() || video.is_some() || !comment.trim().is_empty())
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    fn receipt(now: Instant, token: &str) -> Option<Receipt> {
        Some(Receipt {
            token: token.into(),
            connected_at: now,
        })
    }
    #[test]
    fn only_a_successful_entry_is_consumed_once() {
        let now = Instant::now();
        let mut state = State::default();
        state.end(1, Reason::Disconnected, now);
        assert!(state.pop_pending().is_none());
        state.connected(2, receipt(now, "test"), "Call original".into());
        state.end(1, Reason::Disconnected, now);
        assert!(state.pop_pending().is_none());
        state.end(2, Reason::Left, now + Duration::from_secs(43));
        state.end(2, Reason::Disconnected, now);
        state.connected(2, receipt(now, "duplicate"), "Duplicate".into());
        state.end(2, Reason::Left, now);
        let draft = state.pop_pending().unwrap();
        assert_eq!(draft.label, "Call original");
        assert_eq!(draft.duration_seconds, 43);
        assert!(state.pop_pending().is_none());
    }
    #[test]
    fn switches_preserve_prior_context_and_do_not_mix_receipts() {
        let now = Instant::now();
        let mut state = State::default();
        state.connected(1, receipt(now, "first"), "Primeira call".into());
        state.end(1, Reason::Switched, now + Duration::from_secs(20));
        state.connected(2, receipt(now, "second"), "Segunda call".into());
        state.end(2, Reason::Disconnected, now + Duration::from_secs(4));
        let mut first = state.pop_pending().unwrap();
        first.audio_rating = Some(4);
        let request = first.submit("").unwrap();
        assert_eq!(request.token, "first");
        assert_eq!(request.reason, "switched");
        assert_eq!(first.label, "Primeira call");
        let second = state.pop_pending().unwrap();
        assert_eq!(second.token, "second");
        assert_eq!(second.duration_seconds, 4);
    }
    #[test]
    fn retry_reuses_the_first_payload_and_success_cannot_submit_twice() {
        let now = Instant::now();
        let mut state = State::default();
        state.connected(1, receipt(now, "receipt"), "Call".into());
        state.end(1, Reason::Left, now);
        let mut draft = state.pop_pending().unwrap();
        draft.video_rating = Some(5);
        let first = draft
            .submit("  Vídeo ótimo\nÁudio não avaliado 🎧  ")
            .unwrap();
        assert!(draft.submit("duplicate").is_none());
        draft.completed(Err("Sem conexão".into()));
        let retry = draft
            .submit("changed text must not replace the original")
            .unwrap();
        assert_eq!(first.comment, retry.comment);
        assert_eq!(retry.audio_rating, None);
        assert_eq!(retry.video_rating, Some(5));
        draft.completed(Ok(()));
        assert!(draft.submit("again").is_none());
    }
    #[test]
    fn optional_categories_and_unicode_limits_do_not_invent_a_rating() {
        assert!(!valid_answer(None, None, " \n\t"));
        assert!(valid_answer(Some(1), None, ""));
        assert!(valid_answer(None, None, "Só comentário"));
        assert!(!valid_answer(Some(0), None, "texto"));
        assert!(valid_answer(None, Some(5), &"🎧".repeat(2000)));
        assert!(!valid_answer(None, None, &"🎧".repeat(2001)));
        assert!(!valid_answer(None, None, "bad\0text"));
    }
    #[test]
    fn a_connected_event_queued_before_leave_survives_the_ui_generation_change() {
        let now = Instant::now();
        let mut state = State::default();
        state.starting(1, "Call anterior".into());
        state.end(1, Reason::Left, now + Duration::from_secs(5));
        state.starting(2, "Call atual".into());
        state.connected(2, receipt(now, "current"), "ignored label".into());
        state.late_connected(1, receipt(now, "earlier"));
        let earlier = state.pop_pending().unwrap();
        assert_eq!(earlier.label, "Call anterior");
        assert_eq!(earlier.duration_seconds, 5);
        state.late_connected(1, receipt(now, "duplicate"));
        assert!(state.pop_pending().is_none());
        state.end(2, Reason::Left, now);
        assert_eq!(state.pop_pending().unwrap().token, "current");
    }
    #[test]
    fn connection_completed_after_cancellation_is_not_a_used_session() {
        let now = Instant::now();
        let mut state = State::default();
        state.starting(1, "Cancelled".into());
        state.end(1, Reason::Left, now);
        state.late_connected(1, receipt(now + Duration::from_secs(1), "late"));
        assert!(state.pop_pending().is_none());
    }
    #[test]
    fn an_old_server_without_receipts_never_opens_a_form() {
        let now = Instant::now();
        let mut state = State::default();
        state.connected(1, None, "Call".into());
        state.end(1, Reason::Left, now);
        assert!(state.pop_pending().is_none());
    }
    #[test]
    fn sharing_feedback_reuses_the_entry_receipt_without_a_second_prompt_on_exit() {
        let now = Instant::now();
        let mut state = State::default();
        assert!(!state.can_prompt_sharing(1));
        state.connected(
            1,
            receipt(now, "active-receipt"),
            "Call em andamento".into(),
        );
        assert!(!state.request_sharing(2, now));
        assert!(state.request_sharing(1, now + Duration::from_secs(85)));
        assert!(!state.can_prompt_sharing(1));
        state.end(1, Reason::Left, now + Duration::from_secs(100));
        let mut draft = state.pop_pending().unwrap();
        assert!(draft.during_sharing());
        let request = draft.submit("Transmissão fluida").unwrap();
        assert_eq!(request.token, "active-receipt");
        assert_eq!(request.reason, "sharing");
        assert_eq!(request.duration_seconds, 85);
        assert!(state.pop_pending().is_none());
    }
    #[test]
    fn dismissing_the_small_card_keeps_exit_feedback_and_does_not_nag_in_that_session() {
        let now = Instant::now();
        let mut state = State::default();
        state.connected(1, receipt(now, "first"), "Primeira".into());
        state.dismiss_sharing_prompt(2);
        assert!(state.can_prompt_sharing(1));
        state.dismiss_sharing_prompt(1);
        assert!(!state.can_prompt_sharing(1));
        assert!(!state.request_sharing(1, now));
        state.end(1, Reason::Left, now);
        assert!(!state.pop_pending().unwrap().during_sharing());
        state.connected(2, receipt(now, "second"), "Segunda".into());
        assert!(state.can_prompt_sharing(2));
    }
}
