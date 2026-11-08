//! The words calls-manager shows, in English or Chinese as Denial's
//! `localization.locale` says, else as the system's locale.

use serde_json::Value;

const CHINESE: &[(&str, &str)] = &[
    ("Incoming call", "来电"),
    ("Calling…", "正在呼叫…"),
    ("On hold", "保持中"),
    ("Ongoing call", "通话中"),
    ("Unknown caller", "未知来电"),
    ("Decline", "拒接"),
    ("Answer", "接听"),
    ("Hang up", "挂断"),
];

#[derive(Clone, Copy)]
pub struct Words {
    chinese: bool,
}

impl Words {
    pub fn new(settings: &Value) -> Self {
        let chinese = match settings["localization"]["locale"].as_str() {
            Some("english") => false,
            Some("simplifiedChinese") => true,
            _ => ["LC_ALL", "LC_MESSAGES", "LANG"]
                .into_iter()
                .filter_map(std::env::var_os)
                .find(|value| !value.is_empty())
                .is_some_and(|locale| locale.to_string_lossy().starts_with("zh")),
        };
        Self { chinese }
    }

    /// `english` in the user's language.
    pub fn tr(self, english: &'static str) -> &'static str {
        if !self.chinese {
            return english;
        }
        CHINESE
            .iter()
            .find(|(word, _)| *word == english)
            .map_or(english, |(_, chinese)| chinese)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follows_denial_then_the_system() {
        let chinese = serde_json::json!({ "localization": { "locale": "simplifiedChinese" } });
        assert_eq!(Words::new(&chinese).tr("Answer"), "接听");
        let english = serde_json::json!({ "localization": { "locale": "english" } });
        assert_eq!(Words::new(&english).tr("Answer"), "Answer");
    }
}
