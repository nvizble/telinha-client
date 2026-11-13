//! A guest name is a local display preference, never an account or identity.
use std::{ops::Range, path::PathBuf};

/// UTF-8 storage with UTF-16 conversion at the platform input boundary.
#[derive(Default)]
pub struct NameInput {
    pub text: String,
    pub selection: Range<usize>,
    pub reversed: bool,
    pub marked: Option<Range<usize>>,
}

impl NameInput {
    pub fn new(text: String) -> Self {
        let end = text.len();
        Self {
            text,
            selection: end..end,
            ..Self::default()
        }
    }

    pub fn cursor(&self) -> usize {
        if self.reversed {
            self.selection.start
        } else {
            self.selection.end
        }
    }

    pub fn move_to(&mut self, offset: usize, extend: bool) {
        let mut offset = offset.min(self.text.len());
        while !self.text.is_char_boundary(offset) {
            offset -= 1;
        }
        let anchor = if self.reversed {
            self.selection.end
        } else {
            self.selection.start
        };
        self.selection = if extend {
            anchor.min(offset)..anchor.max(offset)
        } else {
            offset..offset
        };
        self.reversed = extend && offset < anchor;
        self.marked = None;
    }

    pub fn previous(&self, offset: usize) -> usize {
        self.text
            .char_indices()
            .rev()
            .find_map(|(at, _)| (at < offset).then_some(at))
            .unwrap_or(0)
    }

    pub fn next(&self, offset: usize) -> usize {
        self.text
            .char_indices()
            .find_map(|(at, _)| (at > offset).then_some(at))
            .unwrap_or(self.text.len())
    }

    pub fn to_utf16(&self, range: Range<usize>) -> Range<usize> {
        self.text[..range.start].encode_utf16().count()
            ..self.text[..range.end].encode_utf16().count()
    }

    pub fn from_utf16(&self, range: Range<usize>) -> Range<usize> {
        offset_from_utf16(&self.text, range.start)..offset_from_utf16(&self.text, range.end)
    }

    /// Limit the editable buffer independently from final name validation.
    /// Do not silently truncate a paste into a different display name.
    pub fn replace(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        marked: Option<Range<usize>>,
    ) -> bool {
        let range = range
            .or_else(|| self.marked.clone())
            .unwrap_or_else(|| self.selection.clone());
        if range.start > range.end
            || range.end > self.text.len()
            || !self.text.is_char_boundary(range.start)
            || !self.text.is_char_boundary(range.end)
            || text.chars().any(char::is_control)
            || self.text.len() - (range.end - range.start) + text.len() > 512
        {
            return false;
        }
        let selection = marked.as_ref().map(|selection| {
            range.start + offset_from_utf16(text, selection.start)
                ..range.start + offset_from_utf16(text, selection.end)
        });
        self.text.replace_range(range.clone(), text);
        let end = range.start + text.len();
        self.selection = selection.unwrap_or(end..end);
        self.reversed = false;
        self.marked = marked
            .filter(|_| !text.is_empty())
            .map(|_| range.start..end);
        true
    }
}

fn offset_from_utf16(text: &str, offset: usize) -> usize {
    let mut units = 0;
    for (at, ch) in text.char_indices() {
        if units >= offset {
            return at;
        }
        units += ch.len_utf16();
    }
    text.len()
}

fn preference_path() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    let directory = PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("Telinha");
    #[cfg(target_os = "macos")]
    let directory =
        PathBuf::from(std::env::var_os("HOME")?).join("Library/Application Support/tech.telinha");
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let directory = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?
        .join("telinha");
    Some(directory.join("guest-name.json"))
}

pub fn load_name() -> Option<String> {
    let path = preference_path()?;
    if std::fs::metadata(&path).ok()?.len() > 1024 {
        return None;
    }
    let text: String = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    telinha_shared::normalize_guest_name(&text).ok()
}

pub fn save_name(name: &str) -> std::io::Result<()> {
    let name = telinha_shared::normalize_guest_name(name).map_err(std::io::Error::other)?;
    let path = preference_path()
        .ok_or_else(|| std::io::Error::other("Pasta de preferências indisponível"))?;
    std::fs::create_dir_all(path.parent().expect("preference has parent"))?;
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&temporary, serde_json::to_vec(&name)?)?;
    let result = std::fs::rename(&temporary, &path);
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicode_selection_and_ime_replace_the_marked_text() {
        let mut input = NameInput::new("João 🎮".into());
        assert_eq!(input.to_utf16(0..input.text.len()), 0..7);
        assert_eq!(&input.text[input.from_utf16(5..7)], "🎮");
        input.selection = input.from_utf16(5..7);
        assert!(input.replace(None, "に", Some(1..1)));
        assert_eq!(input.text, "João に");
        assert!(input.replace(None, "日本", Some(2..2)));
        assert_eq!(input.text, "João 日本");
        assert_eq!(input.selection, input.text.len()..input.text.len());
        assert!(input.replace(None, "日本", None));
        assert!(input.marked.is_none());
    }

    #[test]
    fn editing_never_slices_through_unicode_or_silently_truncates_paste() {
        let mut input = NameInput::new("João 🎮".into());
        input.move_to(input.previous(input.cursor()), true);
        assert!(input.replace(None, "", None));
        assert_eq!(input.text, "João ");
        assert!(!input.replace(None, "\nPablo", None));
        assert!(!input.replace(None, &"x".repeat(513), None));
        assert_eq!(input.text, "João ");
        input.move_to(0, false);
        input.move_to(input.text.len(), true);
        assert!(input.replace(None, "Bia", None));
        assert_eq!(input.text, "Bia");
    }
}
