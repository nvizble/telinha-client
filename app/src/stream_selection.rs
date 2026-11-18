//! Keep the viewer's chosen publisher across temporary subscription changes.
use crate::session::{RemoteStream, VideoKind};

#[derive(Default)]
pub struct Selection {
    participant: Option<String>,
    track: Option<String>,
    kind: Option<VideoKind>,
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
        self.kind = stream.map(|s| s.kind);
    }

    pub fn added(&mut self, stream: &RemoteStream) -> bool {
        if stream.kind != self.kind.unwrap_or(VideoKind::Screen)
            || self.track.as_ref().is_some_and(|track| track != &stream.id)
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

    /// A replacement may be subscribed before the old track is removed. Recover
    /// only a unique track from the chosen owner/kind, never another broadcaster.
    pub fn recover<'a>(
        &mut self,
        streams: impl IntoIterator<Item = &'a RemoteStream>,
        removed: &str,
    ) {
        if self.track.is_some() || self.participant.is_none() {
            return;
        }
        let candidates: Vec<_> = streams
            .into_iter()
            .filter(|stream| {
                stream.id != removed
                    && self.participant.as_ref() == Some(&stream.participant)
                    && self.kind == Some(stream.kind)
            })
            .collect();
        if let [stream] = candidates.as_slice() {
            self.choose(Some(stream));
        }
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

    #[test]
    fn an_explicit_camera_focus_follows_only_that_person_and_media_kind() {
        let mut focused = Selection::default();
        let mut camera = screen("cam-a1", "alice");
        camera.kind = VideoKind::Camera;
        assert!(!focused.added(&camera));
        focused.choose(Some(&camera));
        assert!(focused.removed("cam-a1"));
        assert!(!focused.added(&screen("screen-a", "alice")));
        let mut other = camera.clone();
        other.id = "cam-b".into();
        other.participant = "bob".into();
        assert!(!focused.added(&other));
        assert_eq!(focused.track(), None);
        camera.id = "cam-a2".into();
        assert!(focused.added(&camera));
        assert_eq!(focused.track(), Some("cam-a2"));
        assert!(!focused.removed("cam-a1"));
        focused.choose(None);
        assert!(!focused.added(&camera));
    }

    #[test]
    fn an_overlapping_subscription_does_not_replace_a_live_choice() {
        let mut selection = Selection::default();
        selection.choose(Some(&screen("a1", "alice")));
        assert!(!selection.added(&screen("a2", "alice")));
        assert_eq!(selection.track(), Some("a1"));
        assert!(selection.removed("a1"));
        let streams = [
            screen("a1", "alice"),
            screen("b1", "bob"),
            screen("a2", "alice"),
        ];
        selection.recover(&streams, "a1");
        assert_eq!(selection.track(), Some("a2"));
        selection.choose(Some(&screen("b1", "bob")));
        selection.recover(&streams, "a2");
        assert_eq!(selection.track(), Some("b1"));
    }
}
