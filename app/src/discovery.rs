use telinha_shared::{CallStatus, DiscordCall};

/// Routing policy is independent of GPUI and media. It never starts capture.
pub struct AutoJoin {
    pub enabled: bool,
    pub active: Option<DiscordCall>,
    held: Option<String>,
    attempted: Option<String>,
}

impl Default for AutoJoin {
    fn default() -> Self {
        Self {
            enabled: true,
            active: None,
            held: None,
            attempted: None,
        }
    }
}

impl AutoJoin {
    pub fn next(&mut self, status: &CallStatus, home: bool, busy: bool) -> Option<DiscordCall> {
        let CallStatus::InCall { call } = status else {
            if matches!(status, CallStatus::Waiting) {
                self.held = None;
                self.attempted = None;
            }
            return None;
        };
        if !home
            && self
                .active
                .as_ref()
                .is_some_and(|active| active.room == call.room)
        {
            self.active = Some(call.clone());
            return None;
        }
        if !self.enabled
            || busy
            || self.held.as_deref() == Some(&call.room)
            || self.attempted.as_deref() == Some(&call.room)
        {
            return None;
        }
        // A manually opened session always keeps priority over discovery.
        if !home && self.active.is_none() {
            return None;
        }
        self.enter(call.clone());
        Some(call.clone())
    }

    pub fn enter(&mut self, call: DiscordCall) {
        self.held = None;
        self.attempted = Some(call.room.clone());
        self.active = Some(call);
    }

    pub fn manual(&mut self) {
        self.active = None;
    }

    pub fn leave(&mut self, current: Option<&CallStatus>) {
        self.held = self
            .active
            .take()
            .map(|call| call.room)
            .or_else(|| match current {
                Some(CallStatus::InCall { call }) => Some(call.room.clone()),
                _ => None,
            });
        self.attempted = None;
    }

    pub fn toggle(&mut self) {
        self.enabled = !self.enabled;
        if self.enabled {
            self.held = None;
            self.attempted = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use telinha_shared::CallUnavailable;

    fn call(room: &str) -> CallStatus {
        CallStatus::InCall {
            call: DiscordCall {
                guild_id: "1".into(),
                guild_name: "Amigos".into(),
                channel_id: room.into(),
                channel_name: "Call".into(),
                room: room.into(),
                participants: vec![],
            },
        }
    }

    #[test]
    fn joins_once_and_does_not_retry_a_failed_room_forever() {
        let mut auto = AutoJoin::default();
        let status = call("aaaaaaaaaaaa");
        assert!(auto.next(&status, true, false).is_some());
        assert!(auto.next(&status, true, false).is_none());
        assert!(
            auto.next(
                &CallStatus::Unavailable {
                    reason: CallUnavailable::Connecting
                },
                true,
                false
            )
            .is_none()
        );
        assert!(auto.next(&status, true, false).is_none());
    }

    #[test]
    fn manual_sessions_and_active_capture_survive_discovery_changes() {
        let mut auto = AutoJoin::default();
        assert!(auto.next(&call("aaaaaaaaaaaa"), false, false).is_none());
        auto.next(&call("aaaaaaaaaaaa"), true, false).unwrap();
        assert!(auto.next(&CallStatus::Waiting, false, true).is_none());
        assert_eq!(auto.active.as_ref().unwrap().room, "aaaaaaaaaaaa");
        assert!(auto.next(&call("bbbbbbbbbbbb"), false, true).is_none());
        assert_eq!(auto.active.as_ref().unwrap().room, "aaaaaaaaaaaa");
        assert_eq!(
            auto.next(&call("bbbbbbbbbbbb"), false, false).unwrap().room,
            "bbbbbbbbbbbb"
        );
    }

    #[test]
    fn explicit_leave_stays_home_until_a_new_call_or_explicit_resume() {
        let mut auto = AutoJoin::default();
        let status = call("aaaaaaaaaaaa");
        auto.next(&status, true, false).unwrap();
        auto.leave(Some(&status));
        assert!(auto.next(&status, true, false).is_none());
        auto.next(&CallStatus::Waiting, true, false);
        assert!(auto.next(&status, true, false).is_some());
        auto.toggle();
        assert!(auto.next(&call("bbbbbbbbbbbb"), true, false).is_none());
        auto.toggle();
        assert!(auto.next(&call("bbbbbbbbbbbb"), true, false).is_some());
    }

    #[test]
    fn multiple_calls_require_selection() {
        let mut auto = AutoJoin::default();
        let CallStatus::InCall { call: first } = call("aaaaaaaaaaaa") else {
            unreachable!()
        };
        let CallStatus::InCall { call: second } = call("bbbbbbbbbbbb") else {
            unreachable!()
        };
        let status = CallStatus::Multiple {
            calls: vec![first.clone(), second],
        };
        assert!(auto.next(&status, true, false).is_none());
        auto.enter(first);
        assert_eq!(auto.active.as_ref().unwrap().room, "aaaaaaaaaaaa");
    }
}
