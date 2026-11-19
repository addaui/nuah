// SPDX-License-Identifier: Apache-2.0
//! Text as the text index sees it ([ADR 0092]): the same folding for what is stored and for what is searched, so a
//! person finds Arabic as they type it, with or without diacritics, and any text in any case.
//!
//! [ADR 0092]: https://docs.addaui.com/nuah/adr/0092-one-index-of-entities-plugins-announce-them-in-their-events-the-data-port-searches-text-and-permissions-follow-the-events-audience/

use serde_json::Value;

/// Lower case; Arabic diacritics (U+064B to U+065F, U+0670) and tatweel removed; أ إ آ ٱ to ا, ى to ي, ة to ه.
pub fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\u{064B}'..='\u{065F}' | '\u{0670}' | '\u{0640}' => {}
            'أ' | 'إ' | 'آ' | 'ٱ' => out.push('ا'),
            'ى' => out.push('ي'),
            'ة' => out.push('ه'),
            c => out.extend(c.to_lowercase()),
        }
    }
    out
}

/// The words of a query, folded, without repeats, in the order given.
pub fn words(q: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for w in fold(q).split_whitespace() {
        if !out.iter().any(|x| x == w) {
            out.push(w.to_string());
        }
    }
    out
}

/// What the index holds for one Document: the folded text of its search fields (a list of strings counts as its
/// items), one field per line. Fields that are missing or hold something else add nothing.
pub fn searchable(body: &Value, fields: &[String]) -> String {
    let mut parts: Vec<String> = vec![];
    for f in fields {
        match body.get(f) {
            Some(Value::String(s)) => parts.push(fold(s)),
            Some(Value::Array(items)) => parts.extend(items.iter().filter_map(|i| i.as_str()).map(fold)),
            _ => {}
        }
    }
    parts.join("\n")
}

/// Does folded text hold every word? (Words come from [`words`].)
pub fn holds_all(text: &str, words: &[String]) -> bool {
    words.iter().all(|w| text.contains(w.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn arabic_is_found_as_people_type_it() {
        assert_eq!(fold("مُحَمَّد"), "محمد", "diacritics go");
        assert_eq!(fold("أحمد إبراهيم آمنة ٱلله"), "احمد ابراهيم امنه الله", "every alef is one alef, final taa marbuta is haa");
        assert_eq!(fold("مصطفى"), "مصطفي");
        assert_eq!(fold("جـــميل"), "جميل", "tatweel goes");
    }

    #[test]
    fn any_case_and_repeated_words() {
        assert_eq!(fold("Release PLAN"), "release plan");
        assert_eq!(fold("İstanbul ΣΟΦΙΑ"), "i̇stanbul σοφια");
        assert_eq!(words("  Plan  plan release "), vec!["plan".to_string(), "release".to_string()]);
    }

    #[test]
    fn searchable_text_takes_strings_and_lists_of_strings() {
        let b = json!({"title": "Release Plan", "aliases": ["RP", 3, "خطة"], "n": 4});
        assert_eq!(searchable(&b, &["title".into(), "aliases".into(), "n".into(), "gone".into()]), "release plan\nrp\nخطه");
        assert!(holds_all(&searchable(&b, &["title".into()]), &words("plan REL")));
        assert!(!holds_all("release plan", &words("plan x")));
    }
}
