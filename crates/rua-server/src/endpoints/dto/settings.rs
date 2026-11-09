use serde_json::{Map, Value};

use crate::endpoints::dto::LanguageSetting;

pub fn language_from_settings(map: &Map<String, Value>) -> LanguageSetting {
    let language = map
        .get("language")
        .and_then(|v| v.as_str())
        .unwrap_or("zh")
        .to_string();
    LanguageSetting {
        language,
        extra: Map::new(),
    }
}
