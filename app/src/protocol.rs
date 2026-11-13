pub fn register() -> anyhow::Result<()> {
    #[cfg(target_os = "windows")]
    {
        use winreg::{RegKey, enums::HKEY_CURRENT_USER};
        let exe = std::env::current_exe()?;
        let (protocol, _) =
            RegKey::predef(HKEY_CURRENT_USER).create_subkey("Software\\Classes\\telinha")?;
        protocol.set_value("", &"URL:telinha")?;
        protocol.set_value("URL Protocol", &"")?;
        let (command, _) = protocol.create_subkey("shell\\open\\command")?;
        command.set_value("", &format!("\"{}\" \"%1\"", exe.display()))?;
    }
    // On macOS LaunchServices reads CFBundleURLTypes from the .app Info.plist.
    Ok(())
}
