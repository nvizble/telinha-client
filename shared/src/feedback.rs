//! Optional feedback about a completed session. Receipts are bearer secrets.
use serde::{Deserialize, Serialize};

pub const MAX_FEEDBACK_COMMENT_CHARS: usize = 2000;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FeedbackRequest {
    pub token: String,
    pub audio_rating: Option<u8>,
    pub video_rating: Option<u8>,
    #[serde(default)]
    pub comment: String,
    pub client: String,
    pub app_version: String,
    pub duration_seconds: u64,
    pub reason: String,
}

impl FeedbackRequest {
    /// Validate content without assuming any particular opaque receipt format.
    pub fn validate(&self) -> Result<(), &'static str> {
        if [self.audio_rating, self.video_rating]
            .into_iter()
            .flatten()
            .any(|rating| !(1..=5).contains(&rating))
        {
            return Err("As notas de áudio e imagem devem ser de 1 a 5.");
        }
        if self.comment.chars().count() > MAX_FEEDBACK_COMMENT_CHARS {
            return Err("Escreva um comentário com até 2000 caracteres.");
        }
        if self
            .comment
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
        {
            return Err("O comentário contém caracteres de controle inválidos.");
        }
        if self.audio_rating.is_none()
            && self.video_rating.is_none()
            && self.comment.trim().is_empty()
        {
            return Err("Dê uma nota ou escreva um comentário para enviar.");
        }
        if !matches!(self.client.as_str(), "windows" | "macos" | "web") {
            return Err("O tipo de aplicativo informado é inválido.");
        }
        if self.app_version.is_empty()
            || self.app_version.len() > 64
            || !self
                .app_version
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-' | b'+' | b'_'))
        {
            return Err("A versão do aplicativo informada é inválida.");
        }
        if self.duration_seconds > i64::MAX as u64 {
            return Err("A duração da sessão informada é inválida.");
        }
        if !matches!(
            self.reason.as_str(),
            "left" | "switched" | "disconnected" | "sharing"
        ) {
            return Err("O momento da avaliação informado é inválido.");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FeedbackResponse {
    pub id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> FeedbackRequest {
        FeedbackRequest {
            token: "opaque".into(),
            audio_rating: None,
            video_rating: None,
            comment: String::new(),
            client: "web".into(),
            app_version: "0.9.0".into(),
            duration_seconds: 0,
            reason: "left".into(),
        }
    }

    #[test]
    fn optional_ratings_never_become_zero_and_unicode_comments_have_a_character_limit() {
        let mut feedback = request();
        assert!(feedback.validate().is_err());
        feedback.audio_rating = Some(5);
        assert!(feedback.validate().is_ok());
        for invalid in [0, 6, 255] {
            feedback.audio_rating = Some(invalid);
            assert!(feedback.validate().is_err());
        }
        feedback.audio_rating = None;
        feedback.video_rating = Some(1);
        assert!(feedback.validate().is_ok());
        feedback.reason = "sharing".into();
        assert!(feedback.validate().is_ok());
        feedback.reason = "unknown".into();
        assert!(feedback.validate().is_err());
        feedback.reason = "left".into();
        feedback.video_rating = None;
        feedback.comment = " Boa imagem\n\tSom baixo\r\n".into();
        assert!(feedback.validate().is_ok());
        feedback.comment = "😀".repeat(MAX_FEEDBACK_COMMENT_CHARS);
        assert!(feedback.validate().is_ok());
        feedback.comment.push('á');
        assert!(feedback.validate().is_err());
        feedback.comment = "texto\0".into();
        assert!(feedback.validate().is_err());
        feedback.comment = " \n\t".into();
        assert!(feedback.validate().is_err());
    }

    #[test]
    fn wire_format_is_camel_case_and_rejects_client_supplied_identity() {
        let mut feedback = request();
        feedback.video_rating = Some(4);
        let mut json = serde_json::to_value(&feedback).unwrap();
        assert!(json["audioRating"].is_null());
        assert_eq!(json["videoRating"], 4);
        assert_eq!(json["appVersion"], "0.9.0");
        json["name"] = "Nome forjado".into();
        assert!(serde_json::from_value::<FeedbackRequest>(json).is_err());
        let old = serde_json::json!({"url":"ws://localhost:7880","token":"jwt","room":"k7f2qxmn3dpa","identity":"guest","expiresIn":21600});
        assert!(
            serde_json::from_value::<crate::SessionCredentials>(old)
                .unwrap()
                .feedback_token
                .is_none()
        );
    }
}
