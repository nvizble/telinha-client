//! Audio applications which would send a call back to its participants.
pub(super) fn blocked_executable(path: &str) -> bool {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    [
        "discord.exe",
        "discordcanary.exe",
        "discordptb.exe",
        "discorddevelopment.exe",
        "telinha.exe",
        // Their process trees can contain unrelated applications, including a call.
        "explorer.exe",
        "applicationframehost.exe",
    ]
    .iter()
    .any(|blocked| name.eq_ignore_ascii_case(blocked))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_call_clients_by_executable_not_window_title() {
        for path in [
            r"C:\Users\a\Discord\app-1\Discord.exe",
            r"C:\Apps\DISCORDPTB.EXE",
            "DiscordCanary.exe",
            "DiscordDevelopment.exe",
            "telinha.exe",
            "explorer.exe",
        ] {
            assert!(blocked_executable(path), "{path}");
        }
        for path in [
            r"C:\Games\Discord racing\game.exe",
            "game.exe",
            "chrome.exe",
        ] {
            assert!(!blocked_executable(path), "{path}");
        }
    }
}
