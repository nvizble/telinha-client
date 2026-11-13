//! One visible person per signed account; guest connections stay separate.
use std::collections::BTreeMap;
use telinha_shared::{UserProfile, discord_user_id, session_profile};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Person {
    pub id: String,
    pub name: String,
    pub profile: Option<UserProfile>,
    pub local: bool,
    pub screen: bool,
    pub camera: bool,
}

pub struct Peer {
    pub identity: String,
    pub name: String,
    pub metadata: String,
    pub local: bool,
    pub screen: bool,
    pub camera: bool,
}

pub fn roster(peers: impl IntoIterator<Item = Peer>) -> Vec<Person> {
    let mut peers: Vec<_> = peers.into_iter().collect();
    peers.sort_by(|a, b| b.local.cmp(&a.local).then(a.identity.cmp(&b.identity)));
    let mut people = BTreeMap::<String, Person>::new();
    for peer in peers {
        let id = discord_user_id(&peer.identity)
            .map(|id| format!("discord:{id}"))
            .unwrap_or_else(|| peer.identity.clone());
        let profile = session_profile(&peer.identity, &peer.metadata);
        let fallback = peer
            .name
            .chars()
            .filter(|c| !c.is_control())
            .take(64)
            .collect::<String>();
        let name = profile
            .as_ref()
            .map(|p| p.display_name.clone())
            .unwrap_or_else(|| {
                if fallback.trim().is_empty() {
                    "Convidado".into()
                } else {
                    fallback
                }
            });
        let person = people.entry(id.clone()).or_insert_with(|| Person {
            id,
            name: name.clone(),
            profile: profile.clone(),
            local: false,
            screen: false,
            camera: false,
        });
        if person.profile.is_none() && profile.is_some() {
            person.profile = profile;
            person.name = name;
        }
        person.local |= peer.local;
        person.screen |= peer.screen;
        person.camera |= peer.camera;
    }
    let mut people: Vec<_> = people.into_values().collect();
    people.sort_by(|a, b| {
        b.local
            .cmp(&a.local)
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then(a.id.cmp(&b.id))
    });
    people
}

#[cfg(test)]
mod tests {
    use super::*;
    fn peer(identity: &str, id: &str, local: bool, screen: bool, camera: bool) -> Peer {
        Peer {
            identity: identity.into(),
            name: String::new(),
            local,
            screen,
            camera,
            metadata: serde_json::to_string(&UserProfile {
                id: id.into(),
                username: "joao".into(),
                display_name: "João".into(),
                avatar_url: "https://cdn.discordapp.com/embed/avatars/0.png".into(),
            })
            .unwrap(),
        }
    }
    #[test]
    fn two_devices_merge_media_but_guests_and_other_accounts_stay_separate() {
        let people = roster([
            peer("discord:123:b", "123", false, true, false),
            peer("guest-a", "123", false, false, false),
            peer("discord:123:a", "123", true, false, true),
            peer("guest-b", "123", false, false, false),
            peer("discord:456:a", "456", false, false, false),
        ]);
        assert_eq!(people.len(), 4);
        assert!(people[0].local && people[0].screen && people[0].camera);
        assert_eq!(people[0].name, "João");
        assert_eq!(people.iter().filter(|p| p.profile.is_some()).count(), 2);
        assert_eq!(people.iter().filter(|p| p.name == "Convidado").count(), 2);
    }
    #[test]
    fn matching_guest_names_do_not_merge_people_or_create_discord_profiles() {
        let mut first = peer("guest-a", "123", false, true, false);
        first.name = "Alex".into();
        let mut second = peer("guest-b", "123", true, false, true);
        second.name = "Alex".into();
        let people = roster([first, second]);
        assert_eq!(people.len(), 2);
        assert!(
            people
                .iter()
                .all(|person| person.name == "Alex" && person.profile.is_none())
        );
        assert_ne!(people[0].id, people[1].id);
        assert_eq!(people.iter().filter(|person| person.screen).count(), 1);
        assert_eq!(people.iter().filter(|person| person.camera).count(), 1);
    }

    #[test]
    fn leaving_and_stopping_a_track_remove_the_previous_state() {
        let current = roster([peer("discord:123:a", "123", true, false, true)]);
        assert_eq!(current.len(), 1);
        assert!(!current[0].screen);
        assert!(roster(Vec::new()).is_empty());
        assert!(
            roster([peer("discord:123:a", "999", true, false, false)])[0]
                .profile
                .is_none()
        );
    }
}
