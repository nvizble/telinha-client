//! Keep the viewer's chosen publisher across temporary subscription changes.
use crate::session::{RemoteStream, VideoKind};

#[derive(Default)]
pub struct Selection {
    participant: Option<String>,
    track: Option<String>,
}

impl Selection {
    pub fn track(&self) -> Option<&str> {
        self.track.as_deref()
    }

    pub fn participant(&self) -> Option<&str> {
        self.participant.as_deref()
    }

    pub fn choose(&mut self, stream: Option<&RemoteStream>) {
        self.participant = stream.map(|s| s.participant.clone());
        self.track = stream.map(|s| s.id.clone());
    }

    pub fn added(&mut self, stream: &RemoteStream) -> bool {
        if stream.kind != VideoKind::Screen
            || self
                .participant
                .as_ref()
                .is_some_and(|participant| participant != &stream.participant)
        {
            return false;
        }
        let changed = self.track.as_deref() != Some(&stream.id);
        self.choose(Some(stream));
        changed
    }

    pub fn removed(&mut self, track: &str) -> bool {
        if self.track.as_deref() != Some(track) {
            return false;
        }
        // Remember the owner even when the SDK removes all publications during
        // a full reconnect. Another publisher must never become the fallback.
        self.track = None;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(id: &str, participant: &str) -> RemoteStream {
        RemoteStream {
            id: id.into(),
            participant: participant.into(),
            title: "Same display name".into(),
            avatar_url: None,
            kind: VideoKind::Screen,
        }
    }

    #[test]
    fn interleaved_reconnects_do_not_switch_to_another_publisher() {
        let mut selection = Selection::default();
        assert!(selection.added(&screen("a1", "alice")));
        assert!(!selection.added(&screen("b1", "bob")));
        assert!(selection.removed("a1"));
        assert!(!selection.added(&screen("b2", "bob")));
        assert_eq!(selection.track(), None);
        assert_eq!(selection.participant(), Some("alice"));
        assert!(selection.added(&screen("a2", "alice")));
        assert!(!selection.removed("a1"));
        assert_eq!(selection.track(), Some("a2"));
    }

    #[test]
    fn explicit_choice_wins_over_new_tracks_and_republished_cameras() {
        let mut selection = Selection::default();
        selection.added(&screen("a1", "alice"));
        selection.choose(Some(&screen("b1", "bob")));
        assert!(!selection.added(&screen("a2", "alice")));
        let mut camera = screen("camera-b", "bob");
        camera.kind = VideoKind::Camera;
        assert!(!selection.added(&camera));
        assert!(!selection.removed("a1"));
        assert_eq!(selection.track(), Some("b1"));
        assert!(selection.removed("b1"));
        assert!(!selection.added(&screen("c1", "charlie")));
        assert_eq!(selection.track(), None);
        selection.choose(Some(&screen("c1", "charlie")));
        assert!(!selection.added(&screen("b2", "bob")));
        assert_eq!(selection.track(), Some("c1"));
    }

    #[test]
    fn leaving_the_room_resets_the_choice_for_the_next_session() {
        let mut selection = Selection::default();
        selection.added(&screen("a1", "alice"));
        selection.choose(None);
        assert_eq!(selection.participant(), None);
        assert!(selection.added(&screen("b1", "bob")));
        assert_eq!(selection.track(), Some("b1"));
    }
}
