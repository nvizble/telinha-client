use rand::{RngCore, rngs::OsRng};
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};
use thiserror::Error;

pub mod e2ee;
pub mod feedback;
pub mod updates;

pub const CODE_LENGTH: usize = 12;
pub const CODE_ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";

/// The code is the room's bearer capability. Never log it with its token.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct RoomCode(String);

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CodeError {
    #[error("O código tem 12 caracteres. Confira se copiou tudo.")]
    Length,
    #[error("O código só usa letras de a a z e números de 2 a 7.")]
    Character,
    #[error("Esse link não é da telinha. Cole o código da sessão.")]
    Link,
}

impl RoomCode {
    pub fn generate() -> Self {
        let mut bytes = [0u8; CODE_LENGTH];
        OsRng.fill_bytes(&mut bytes);
        // 32 divides 256 exactly: no modulo bias, 60 bits of OS randomness.
        Self(
            bytes
                .iter()
                .map(|n| CODE_ALPHABET[(n & 31) as usize] as char)
                .collect(),
        )
    }

    /// User input may be grouped, upper-case, or a first-party invite link.
    /// HTTP endpoints instead use FromStr, which accepts only the canonical code.
    pub fn from_input(input: &str) -> Result<Self, CodeError> {
        let input = input.trim();
        let raw = if input.contains("://") {
            let url = url::Url::parse(input).map_err(|_| CodeError::Link)?;
            if !url.username().is_empty()
                || url.password().is_some()
                || url.port().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
            {
                return Err(CodeError::Link);
            }
            match url.scheme() {
                "telinha" if url.path().is_empty() || url.path() == "/" => {
                    url.host_str().ok_or(CodeError::Link)?.to_owned()
                }
                "https" if url.host_str() == Some("telinha.tech") => url
                    .path()
                    .strip_prefix("/j/")
                    .ok_or(CodeError::Link)?
                    .to_owned(),
                _ => return Err(CodeError::Link),
            }
        } else {
            input.to_owned()
        };
        let raw = raw.to_ascii_lowercase();
        if raw.contains('-') {
            let groups: Vec<_> = raw.split('-').collect();
            if groups.len() != 3 || groups.iter().any(|g| g.len() != 4) {
                return Err(CodeError::Length);
            }
            groups.concat().parse()
        } else {
            raw.parse()
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn grouped(&self) -> String {
        format!("{}-{}-{}", &self.0[..4], &self.0[4..8], &self.0[8..])
    }
    pub fn invite_url(&self) -> String {
        format!("https://telinha.tech/j/{}", self.0)
    }
    pub fn deep_link(&self) -> String {
        format!("telinha://{}", self.0)
    }
}

impl FromStr for RoomCode {
    type Err = CodeError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.len() != CODE_LENGTH {
            return Err(CodeError::Length);
        }
        if !s.bytes().all(|c| CODE_ALPHABET.contains(&c)) {
            return Err(CodeError::Character);
        }
        Ok(Self(s.to_owned()))
    }
}

impl fmt::Display for RoomCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl fmt::Debug for RoomCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RoomCode([private])")
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionCredentials {
    pub url: String,
    pub token: String,
    pub room: String,
    pub identity: String,
    pub expires_in: u64,
    /// Opaque authorization to relay encrypted key envelopes, never a media key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub e2ee: Option<e2ee::RelayCredentials>,
    /// Optional receipt for feedback; never use the media JWT for this purpose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feedback_token: Option<String>,
}

pub const MAX_GUEST_NAME_CHARS: usize = 40;

/// A display name never supplies an account identity or authorization.
pub fn normalize_guest_name(value: &str) -> Result<String, &'static str> {
    if value.chars().any(char::is_control) {
        return Err("O nome não pode conter quebras de linha ou caracteres de controle.");
    }
    let name = value.trim();
    if name.is_empty() {
        return Err("Informe seu nome para entrar.");
    }
    if name.chars().count() > MAX_GUEST_NAME_CHARS {
        return Err("Use um nome com até 40 caracteres.");
    }
    Ok(name.to_owned())
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UserProfile {
    pub id: String,
    pub username: String,
    pub display_name: String,
    pub avatar_url: String,
}

/// Discord identity comes from the server-issued token, never display metadata.
pub fn discord_user_id(identity: &str) -> Option<&str> {
    let (user, connection) = identity.strip_prefix("discord:")?.split_once(':')?;
    (!user.is_empty()
        && user.bytes().all(|byte| byte.is_ascii_digit())
        && !connection.is_empty()
        && !connection.contains(':'))
    .then_some(user)
}

pub fn session_profile(identity: &str, metadata: &str) -> Option<UserProfile> {
    let id = discord_user_id(identity)?;
    if metadata.len() > 4096 {
        return None;
    }
    let mut profile: UserProfile = serde_json::from_str(metadata).ok()?;
    if profile.id != id {
        return None;
    }
    profile.display_name = profile
        .display_name
        .chars()
        .filter(|c| !c.is_control())
        .take(64)
        .collect::<String>()
        .trim()
        .to_owned();
    profile.username = profile
        .username
        .chars()
        .filter(|c| !c.is_control())
        .take(32)
        .collect();
    if profile.display_name.is_empty() {
        profile.display_name.clone_from(&profile.username);
    }
    if profile.display_name.is_empty() {
        return None;
    }
    let valid_avatar = url::Url::parse(&profile.avatar_url).is_ok_and(|url| {
        url.scheme() == "https"
            && url.host_str() == Some("cdn.discordapp.com")
            && url.port().is_none()
            && url.username().is_empty()
            && url.password().is_none()
            && (url.path().starts_with("/avatars/") || url.path().starts_with("/embed/avatars/"))
    });
    if !valid_avatar {
        profile.avatar_url.clear();
    }
    Some(profile)
}

/// The server signs Discord identities as `discord:<user>:<connection>` so two
/// devices can coexist. Count the account once while keeping guests distinct.
/// Names and mutable participant metadata are never used to merge people.
pub fn people_count<'a>(identities: impl IntoIterator<Item = &'a str>) -> usize {
    identities
        .into_iter()
        .map(|identity| {
            if let Some(user) = discord_user_id(identity) {
                &identity[.."discord:".len() + user.len()]
            } else {
                identity
            }
        })
        .collect::<std::collections::HashSet<_>>()
        .len()
}

// Room codes are capabilities, so discovery responses intentionally omit Debug.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiscordCall {
    pub guild_id: String,
    pub guild_name: String,
    pub channel_id: String,
    pub channel_name: String,
    pub room: String,
    pub participants: Vec<UserProfile>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CallUnavailable {
    Disabled,
    Connecting,
    NotInstalled,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CallStatus {
    Unavailable { reason: CallUnavailable },
    Waiting,
    InCall { call: DiscordCall },
    Multiple { calls: Vec<DiscordCall> },
}

// Credentials deliberately do not implement Debug: never log bearer tokens.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginStart {
    pub browser_url: String,
    pub poll_token: String,
    pub expires_in: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountSession {
    pub token: String,
    pub user: UserProfile,
    pub expires_at: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum LoginStatus {
    Pending,
    Complete { session: AccountSession },
    Failed { message: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quality {
    FullHd30,
    Hd60,
    FullHd60,
}

impl Quality {
    pub fn label(self) -> &'static str {
        match self {
            Self::FullHd30 => "1080p, 30 fps",
            Self::Hd60 => "720p, 60 fps",
            Self::FullHd60 => "1080p, 60 fps",
        }
    }
    pub fn fps(self) -> u32 {
        if self == Self::FullHd30 { 30 } else { 60 }
    }
    pub fn dimensions(self) -> (u32, u32) {
        if self == Self::Hd60 {
            (1280, 720)
        } else {
            (1920, 1080)
        }
    }
    pub fn bitrate(self) -> u64 {
        match self {
            Self::FullHd30 => 4_000_000,
            Self::Hd60 => 3_000_000,
            Self::FullHd60 => 6_000_000,
        }
    }
    pub fn fit(self, w: u32, h: u32) -> (u32, u32) {
        let (max_w, max_h) = self.dimensions();
        let scale = (max_w as f64 / w.max(1) as f64)
            .min(max_h as f64 / h.max(1) as f64)
            .min(1.0);
        (
            ((w as f64 * scale) as u32 & !1).max(2),
            ((h as f64 * scale) as u32 & !1).max(2),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn guest_names_are_trimmed_bounded_unicode_without_control_characters() {
        assert_eq!(
            normalize_guest_name("  João da Silva  ").unwrap(),
            "João da Silva"
        );
        assert_eq!(
            normalize_guest_name("\u{2003}👩🏽‍💻 Ana\u{a0}").unwrap(),
            "👩🏽‍💻 Ana"
        );
        assert_eq!(
            normalize_guest_name(&"🦊".repeat(40)).unwrap(),
            "🦊".repeat(40)
        );
        assert!(normalize_guest_name(&"á".repeat(41)).is_err());
        for invalid in [
            "",
            "   ",
            "\u{a0}\u{2003}",
            "Ana\n",
            "\tAna",
            "An\0a",
            "Ana\u{7f}",
            "Ana\u{85}",
        ] {
            assert!(normalize_guest_name(invalid).is_err(), "{invalid:?}");
        }
        // Validation treats markup as plain display text; renderers must not interpret it.
        assert_eq!(normalize_guest_name("<b>Ana</b>").unwrap(), "<b>Ana</b>");
    }

    #[test]
    fn session_profile_requires_the_signed_account_and_bounds_display_fields() {
        let profile = UserProfile {
            id: "123".into(),
            username: "joao".into(),
            display_name: "João\n\u{1b}".into(),
            avatar_url: "https://example.com/tracker.png".into(),
        };
        let metadata = serde_json::to_string(&profile).unwrap();
        let safe = session_profile("discord:123:connection", &metadata).unwrap();
        assert_eq!(safe.display_name, "João");
        assert!(safe.avatar_url.is_empty());
        for identity in [
            "guest",
            "discord:123",
            "discord:123:a:b",
            "discord:456:connection",
        ] {
            assert!(session_profile(identity, &metadata).is_none());
        }
        assert!(session_profile("discord:123:connection", &" ".repeat(4097)).is_none());
    }
    #[test]
    fn people_count_merges_devices_but_preserves_accounts_and_guests() {
        let mac = "discord:123456:mac-connection";
        let windows = "discord:123456:windows-connection";
        let friend = "discord:654321:friend-connection";
        assert_eq!(people_count([mac, windows]), 1);
        assert_eq!(
            people_count([mac, windows, friend, "guest-1", "guest-2"]),
            4
        );
        assert_eq!(people_count([windows, friend]), 2);
        assert_eq!(people_count([windows]), 1);
        assert_eq!(people_count([]), 0);
        assert_eq!(
            people_count([
                "discord::a",
                "discord::b",
                "discord:name:a",
                "discord:name:b"
            ]),
            4
        );
    }
    #[test]
    fn input_and_canonical_validation_have_distinct_boundaries() {
        let code: RoomCode = "k7f2qxmn3dpa".parse().unwrap();
        for input in [
            " K7F2-QXMN-3DPA ",
            "telinha://k7f2qxmn3dpa",
            "https://telinha.tech/j/k7f2qxmn3dpa",
        ] {
            assert_eq!(RoomCode::from_input(input).unwrap(), code);
        }
        for bad in [
            "k7f2-9xqm-3dpa",
            "k7f29xqm3dpa",
            "K7F2QXMN3DPA",
            "k7f2qxmn3dp",
            "k7f2qxmn3dpa!",
            "éééééé",
        ] {
            assert!(bad.parse::<RoomCode>().is_err());
        }
        for bad in [
            "https://evil.test/j/k7f2qxmn3dpa",
            "https://telinha.tech.evil.test/j/k7f2qxmn3dpa",
            "telinha://user@k7f2qxmn3dpa",
            "telinha://k7f2qxmn3dpa/path",
            "telinha://k7f2qxmn3dpa?token=x",
            "https://telinha.tech/j/k7f2qxmn3dpa#x",
        ] {
            assert!(RoomCode::from_input(bad).is_err(), "{bad}");
        }
    }
    #[test]
    fn random_codes_round_trip_without_collisions_in_sample() {
        let mut codes = std::collections::HashSet::new();
        for _ in 0..1000 {
            let code = RoomCode::generate();
            assert_eq!(RoomCode::from_input(&code.grouped()).unwrap(), code);
            assert!(codes.insert(code));
        }
    }
    #[test]
    fn quality_preserves_aspect_and_does_not_upscale() {
        assert_eq!(Quality::FullHd30.fit(2560, 1440), (1920, 1080));
        assert_eq!(Quality::FullHd30.fit(1080, 1920), (606, 1080));
        assert_eq!(Quality::Hd60.fit(640, 480), (640, 480));
        assert_eq!(Quality::FullHd30.fit(3440, 1440), (1920, 802));
        assert_eq!(Quality::FullHd60.fit(5120, 1440), (1920, 540));
        assert_eq!(Quality::Hd60.fit(3440, 1440), (1280, 534));
    }
}
