use crate::guest::NameInput;
use std::ops::{Deref, DerefMut, Range};
use telinha_shared::feedback::MAX_FEEDBACK_COMMENT_CHARS;
#[derive(Default)]
pub struct CommentInput(NameInput);
impl Deref for CommentInput {
    type Target = NameInput;
    fn deref(&self) -> &NameInput {
        &self.0
    }
}
impl DerefMut for CommentInput {
    fn deref_mut(&mut self) -> &mut NameInput {
        &mut self.0
    }
}
impl CommentInput {
    pub fn replace(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        marked: Option<Range<usize>>,
    ) -> bool {
        let range = range
            .or_else(|| self.marked.clone())
            .unwrap_or_else(|| self.selection.clone());
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        if range.start > range.end
            || range.end > self.text.len()
            || !self.text.is_char_boundary(range.start)
            || !self.text.is_char_boundary(range.end)
            || text
                .chars()
                .any(|ch| ch.is_control() && !matches!(ch, '\n' | '\t'))
        {
            return false;
        }
        let count = self.text[..range.start].chars().count()
            + self.text[range.end..].chars().count()
            + text.chars().count();
        if count > MAX_FEEDBACK_COMMENT_CHARS {
            return false;
        }
        let selection = marked.as_ref().map(|selection| {
            let local = NameInput::new(text.clone()).from_utf16(selection.clone());
            range.start + local.start..range.start + local.end
        });
        self.text.replace_range(range.clone(), &text);
        let end = range.start + text.len();
        self.selection = selection.unwrap_or(end..end);
        self.reversed = false;
        self.marked = marked
            .filter(|_| !text.is_empty())
            .map(|_| range.start..end);
        true
    }
    pub fn line_start(&self) -> usize {
        self.text[..self.cursor()]
            .rfind('\n')
            .map_or(0, |at| at + 1)
    }
    pub fn line_end(&self) -> usize {
        self.text[self.cursor()..]
            .find('\n')
            .map_or(self.text.len(), |at| self.cursor() + at)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn multiline_ime_and_emoji_preserve_selection() {
        let mut input = CommentInput::default();
        assert!(input.replace(None, "Áudio 🎧\r\nVídeo\t", None));
        assert_eq!(input.text, "Áudio 🎧\nVídeo\t");
        let selection = input.from_utf16(6..8);
        input.selection = selection;
        assert!(input.replace(None, "に", Some(1..1)));
        assert!(input.replace(None, "日本", Some(2..2)));
        assert!(input.replace(None, "日本", None));
        assert_eq!(input.text, "Áudio 日本\nVídeo\t");
        assert!(input.marked.is_none());
    }
    #[test]
    fn limit_counts_unicode_characters_and_rejects_paste_atomically() {
        let mut input = CommentInput::default();
        assert!(input.replace(None, &"🎧".repeat(2000), None));
        assert!(!input.replace(None, "x", None));
        assert_eq!(input.text.chars().count(), 2000);
        input.move_to(0, false);
        let end = input.text.len();
        input.move_to(end, true);
        assert!(!input.replace(None, "bad\0text", None));
        assert_eq!(input.text.chars().count(), 2000);
        assert!(input.replace(None, "Primeira\nSegunda", None));
        assert_eq!(input.line_start(), 9);
        assert_eq!(input.line_end(), 16);
    }
}
