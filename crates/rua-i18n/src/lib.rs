use std::collections::HashMap;
use std::sync::OnceLock;

const TRANSLATIONS_YAML: &str = include_str!("../locales/app.yml");
const SUPPORTED_LANGS: &[&str] = &["zh", "en", "ja", "ru"];
const FALLBACK_LANG: &str = "zh";

type TranslationMap = HashMap<String, HashMap<String, String>>;

fn translations() -> &'static TranslationMap {
    static INSTANCE: OnceLock<TranslationMap> = OnceLock::new();
    INSTANCE.get_or_init(|| parse_yaml(TRANSLATIONS_YAML))
}

fn parse_yaml(yaml: &str) -> TranslationMap {
    let mut map = TranslationMap::new();
    let mut current_key: Option<String> = None;
    let mut current_lang: Option<String> = None;
    let mut current_lines: Vec<String> = Vec::new();

    for line in yaml.lines() {
        if line.starts_with('_') || line.trim().is_empty() {
            continue;
        }

        if !line.starts_with(' ') && line.ends_with(':') {
            flush_entry(&mut map, &current_key, &current_lang, &current_lines);
            current_key = Some(line.trim_end_matches(':').to_string());
            current_lang = None;
            current_lines.clear();
            continue;
        }

        if let Some(ref key) = current_key {
            let trimmed = line.trim();
            if trimmed.ends_with(": |") {
                flush_entry(&mut map, &Some(key.clone()), &current_lang, &current_lines);
                current_lang = Some(trimmed.trim_end_matches(": |").to_string());
                current_lines.clear();
            } else if current_lang.is_some() {
                let content = line.strip_prefix("    ").unwrap_or(line);
                current_lines.push(content.to_string());
            }
        }
    }

    flush_entry(&mut map, &current_key, &current_lang, &current_lines);
    map
}

fn flush_entry(
    map: &mut TranslationMap,
    key: &Option<String>,
    lang: &Option<String>,
    lines: &[String],
) {
    if let (Some(k), Some(l)) = (key, lang) {
        if !lines.is_empty() {
            let text = lines.join("\n").trim_end().to_string();
            map.entry(k.clone()).or_default().insert(l.clone(), text);
        }
    }
}

pub fn t(key: &str, lang: &str) -> String {
    let map = translations();
    if let Some(langs) = map.get(key) {
        if let Some(text) = langs.get(lang) {
            return text.clone();
        }
        if let Some(text) = langs.get(FALLBACK_LANG) {
            return text.clone();
        }
    }
    key.to_string()
}

pub fn normalize_lang(raw: &str) -> String {
    let lower = raw.to_lowercase().replace('_', "-");
    for &lang in SUPPORTED_LANGS {
        if lower.starts_with(lang) {
            return lang.to_string();
        }
    }
    FALLBACK_LANG.to_string()
}
