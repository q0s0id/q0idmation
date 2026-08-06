use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
#[derive(Default)]
pub enum Language {
    #[default]
    English,
}

impl Language {
    pub const ALL: &'static [Language] = &[Language::English];

    pub fn label(self) -> &'static str {
        match self {
            Language::English => "English",
        }
    }
}

pub struct Strings {
    pub localization_heading: &'static str,
    pub language_label: &'static str,
}

const ENGLISH: Strings = Strings {
    localization_heading: "Localization",
    language_label: "Language",
};

pub fn strings(language: Language) -> &'static Strings {
    match language {
        Language::English => &ENGLISH,
    }
}
