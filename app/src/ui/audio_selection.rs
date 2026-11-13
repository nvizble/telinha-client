#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ApplicationKey {
    pub window_id: u64,
    pub process_id: u32,
    pub created_at: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Selection {
    Off,
    #[default]
    FollowWindow,
    // None means the user must choose an application, never a fallback source.
    Application(Option<ApplicationKey>),
}

impl Selection {
    /// A display has no owning audio process. Require an explicit application
    /// instead of treating the default follow mode as a successful silent share.
    pub(super) fn select_display(&mut self) -> bool {
        if *self == Self::FollowWindow {
            *self = Self::Application(None);
            return true;
        }
        false
    }

    pub(super) fn resolve(
        self,
        shared_window: Option<u64>,
        available: impl IntoIterator<Item = ApplicationKey>,
    ) -> Option<ApplicationKey> {
        match self {
            Self::Off | Self::Application(None) => None,
            Self::FollowWindow => shared_window.and_then(|window_id| {
                available
                    .into_iter()
                    .find(|candidate| candidate.window_id == window_id)
            }),
            Self::Application(Some(selected)) => available
                .into_iter()
                .find(|candidate| *candidate == selected),
        }
    }

    /// Invalidate a missing selection without silently switching applications.
    /// Even if it returns later, sending its audio requires a fresh choice.
    pub(super) fn refresh(&mut self, available: impl IntoIterator<Item = ApplicationKey>) -> bool {
        if let Self::Application(Some(selected)) = *self
            && !available.into_iter().any(|candidate| candidate == selected)
        {
            *self = Self::Application(None);
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GAME: ApplicationKey = ApplicationKey {
        window_id: 10,
        process_id: 20,
        created_at: 30,
    };
    const MUSIC: ApplicationKey = ApplicationKey {
        window_id: 40,
        process_id: 50,
        created_at: 60,
    };

    #[test]
    fn monitor_requires_an_audio_choice_and_preserves_explicit_preferences() {
        let mut follow = Selection::FollowWindow;
        assert!(follow.select_display());
        assert_eq!(follow, Selection::Application(None));
        for mut preference in [Selection::Off, Selection::Application(Some(GAME))] {
            let before = preference;
            assert!(!preference.select_display());
            assert_eq!(preference, before);
        }
    }

    #[test]
    fn explicit_application_supplies_audio_for_a_monitor() {
        let selection = Selection::Application(Some(GAME));
        assert_eq!(selection.resolve(None, [GAME, MUSIC]), Some(GAME));
        assert_eq!(Selection::Application(None).resolve(None, [GAME]), None);
    }

    #[test]
    fn off_never_uses_an_available_source() {
        assert_eq!(Selection::Off.resolve(Some(GAME.window_id), [GAME]), None);
        assert_eq!(Selection::Off.resolve(None, [GAME]), None);
    }

    #[test]
    fn following_tracks_the_video_window_and_is_silent_for_a_monitor() {
        let selection = Selection::default();
        assert_eq!(
            selection.resolve(Some(GAME.window_id), [MUSIC, GAME]),
            Some(GAME)
        );
        assert_eq!(
            selection.resolve(Some(MUSIC.window_id), [MUSIC, GAME]),
            Some(MUSIC)
        );
        assert_eq!(selection.resolve(None, [MUSIC, GAME]), None);
        assert_eq!(selection.resolve(Some(999), [MUSIC, GAME]), None);
    }

    #[test]
    fn changing_video_does_not_replace_an_explicit_application() {
        let selection = Selection::Application(Some(MUSIC));
        for video in [Some(GAME.window_id), Some(MUSIC.window_id), None] {
            assert_eq!(selection.resolve(video, [GAME, MUSIC]), Some(MUSIC));
        }
    }

    #[test]
    fn refresh_keeps_identity_when_candidates_are_reordered() {
        let mut selection = Selection::Application(Some(MUSIC));
        assert!(!selection.refresh([MUSIC, GAME]));
        assert_eq!(
            selection.resolve(Some(GAME.window_id), [GAME, MUSIC]),
            Some(MUSIC)
        );
    }

    #[test]
    fn recycled_window_or_process_never_replaces_the_selected_application() {
        for stale in [
            ApplicationKey {
                window_id: 999,
                ..GAME
            },
            ApplicationKey {
                process_id: 999,
                ..GAME
            },
            ApplicationKey {
                created_at: 999,
                ..GAME
            },
        ] {
            let mut selection = Selection::Application(Some(GAME));
            assert_eq!(
                selection.resolve(Some(GAME.window_id), [stale, MUSIC]),
                None
            );
            assert!(selection.refresh([stale, MUSIC]));
            assert_eq!(selection, Selection::Application(None));
            assert_eq!(selection.resolve(Some(GAME.window_id), [GAME, MUSIC]), None);
        }
    }

    #[test]
    fn refreshing_does_not_change_off_or_follow_modes() {
        for mut selection in [Selection::Off, Selection::FollowWindow] {
            let before = selection;
            assert!(!selection.refresh([]));
            assert_eq!(selection, before);
        }
    }
}
