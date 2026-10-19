//! Interface languages.
//!
//! Spanish is the source language: every text is written in Spanish in the
//! code, and that exact text is the key into the other languages' tables
//! (`lang/<code>.json`, embedded at build time). A text missing from a table
//! shows in Spanish rather than as an empty label.
//!
//! - `tr("Texto")` translates a literal.
//! - `tf!("Copiado a {dst}", dst = path.display())` translates and then fills
//!   the `{name}` placeholders, so a translation can move them around.
//! - `n_("Texto")` marks a literal that is stored now and translated later
//!   with `tr`, such as the labels in a `const` table.
//!
//! The test at the bottom reads the sources and fails when a marked text has
//! no translation, when a translation no longer has a text, or when the
//! placeholders of the two sides differ. Adding a language is a new
//! `lang/<code>.json` and a line in `LANGS`.

use std::collections::HashMap;
use std::fmt::Display;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;

/// One interface language.
pub struct Lang {
    /// ISO 639-1 code, also the value saved in the preferences.
    pub code: &'static str,
    /// Name in the language itself, for the selector.
    pub name: &'static str,
    /// Translation table, `None` for the source language.
    table: Option<&'static str>,
}

/// Every language the interface speaks. The first one is the source.
pub const LANGS: &[Lang] = &[
    Lang {
        code: "es",
        name: "Español",
        table: None,
    },
    Lang {
        code: "en",
        name: "English",
        table: Some(include_str!("../lang/en.json")),
    },
];

/// Index into `LANGS` of the language in use.
static CURRENT: AtomicUsize = AtomicUsize::new(0);

/// Parsed tables, one per entry of `LANGS`, built on first use.
static TABLES: OnceLock<Vec<HashMap<&'static str, &'static str>>> = OnceLock::new();

fn tables() -> &'static [HashMap<&'static str, &'static str>] {
    TABLES.get_or_init(|| {
        LANGS
            .iter()
            .map(|lang| lang.table.map(parse).unwrap_or_default())
            .collect()
    })
}

/// The tables live for the whole run, so their strings are leaked once and
/// `tr` can hand out `&'static str` without copying on every frame.
fn parse(json: &'static str) -> HashMap<&'static str, &'static str> {
    let map: HashMap<String, String> = serde_json::from_str(json).unwrap_or_default();
    map.into_iter()
        .map(|(k, v)| -> (&'static str, &'static str) { (k.leak(), v.leak()) })
        .collect()
}

/// The language in use.
pub fn current() -> &'static Lang {
    &LANGS[CURRENT.load(Ordering::Relaxed)]
}

/// Switches the interface to `pref`: a code from `LANGS`, or empty to follow
/// the system. Unknown codes follow the system too.
pub fn set(pref: &str) {
    let code = if pref.is_empty() {
        system()
    } else {
        pref.to_string()
    };
    let index = LANGS.iter().position(|l| l.code == code).unwrap_or(0);
    CURRENT.store(index, Ordering::Relaxed);
}

/// Language of the system, reduced to a code from `LANGS`. Spanish systems
/// get Spanish; anything else gets English, which more people read than the
/// source language.
pub fn system() -> String {
    let locale = system_locale().to_ascii_lowercase();
    let code = locale.get(..2).unwrap_or("");
    if LANGS.iter().any(|l| l.code == code) {
        code.to_string()
    } else {
        "en".to_string()
    }
}

#[cfg(windows)]
fn system_locale() -> String {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetUserDefaultLocaleName(name: *mut u16, len: i32) -> i32;
    }
    // LOCALE_NAME_MAX_LENGTH
    let mut buf = [0u16; 85];
    // SAFETY: the buffer and its length match; the call writes at most `len`
    // UTF-16 units including the terminating zero and returns that count.
    let n = unsafe { GetUserDefaultLocaleName(buf.as_mut_ptr(), buf.len() as i32) };
    if n <= 1 {
        return String::new();
    }
    String::from_utf16_lossy(&buf[..n as usize - 1])
}

#[cfg(not(windows))]
fn system_locale() -> String {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|v| std::env::var(v).ok())
        .find(|v| !v.is_empty())
        .unwrap_or_default()
}

/// Translates `s` into the language in use.
pub fn tr(s: &str) -> &str {
    tables()[CURRENT.load(Ordering::Relaxed)]
        .get(s)
        .copied()
        .unwrap_or(s)
}

/// Marks a literal as a text to translate without translating it yet.
pub const fn n_(s: &'static str) -> &'static str {
    s
}

/// Replaces each `{name}` in `template` with its value. `{{` and `}}` stand
/// for literal braces, as in `format!`.
pub fn fill(template: &str, args: &[(&str, &dyn Display)]) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut rest = template;
    while let Some(i) = rest.find(['{', '}']) {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        if tail.starts_with("{{") || tail.starts_with("}}") {
            out.push_str(&tail[..1]);
            rest = &tail[2..];
            continue;
        }
        if let (true, Some(end)) = (tail.starts_with('{'), tail.find('}')) {
            let name = &tail[1..end];
            if let Some((_, value)) = args.iter().find(|(k, _)| *k == name) {
                out.push_str(&value.to_string());
                rest = &tail[end + 1..];
                continue;
            }
        }
        out.push_str(&tail[..1]);
        rest = &tail[1..];
    }
    out.push_str(rest);
    out
}

/// Translates a template and fills its placeholders:
/// `tf!("Copiado a {dst}", dst = path.display())`, or `tf!("... {name}", name)`
/// when a variable of that name is in scope.
#[macro_export]
macro_rules! tf {
    (@v $k:ident) => { $k };
    (@v $k:ident, $v:expr) => { $v };
    ($s:literal $(, $k:ident $(= $v:expr)?)* $(,)?) => {
        $crate::i18n::fill(
            $crate::i18n::tr($s),
            &[$((stringify!($k), &$crate::tf!(@v $k $(, $v)?) as &dyn ::std::fmt::Display)),*],
        )
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};

    #[test]
    fn fill_moves_placeholders_and_keeps_braces() {
        let a = 3;
        assert_eq!(
            fill("{b} y {a} {{x}} {c}", &[("a", &a), ("b", &"dos")]),
            "dos y 3 {x} {c}"
        );
        let name = "x";
        assert_eq!(
            tf!("Proyecto '{name}' cargado.", name),
            "Proyecto 'x' cargado."
        );
    }

    /// Decodes the Rust string literal starting right after its opening
    /// quote. Returns the text and the rest of the source.
    fn literal(src: &str) -> Option<(String, &str)> {
        let mut out = String::new();
        let mut chars = src.char_indices().peekable();
        while let Some((i, c)) = chars.next() {
            match c {
                '"' => return Some((out, &src[i + 1..])),
                '\\' => match chars.next()?.1 {
                    'n' => out.push('\n'),
                    't' => out.push('\t'),
                    '0' => out.push('\0'),
                    '\\' => out.push('\\'),
                    '"' => out.push('"'),
                    '\'' => out.push('\''),
                    'u' => {
                        let mut hex = String::new();
                        for (_, h) in chars.by_ref() {
                            match h {
                                '{' => {}
                                '}' => break,
                                _ => hex.push(h),
                            }
                        }
                        out.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
                    }
                    // A line continuation swallows the newline and the
                    // indentation that follows it.
                    '\n' | '\r' => while chars.next_if(|(_, ch)| ch.is_whitespace()).is_some() {},
                    other => panic!("escape \\{other} not handled"),
                },
                _ => out.push(c),
            }
        }
        None
    }

    /// Every text marked for translation in the sources.
    fn marked() -> BTreeSet<String> {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut keys = BTreeSet::new();
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|e| e != "rs") || path.ends_with("i18n.rs") {
                continue;
            }
            let src = std::fs::read_to_string(&path).unwrap();
            for marker in ["tr(", "tf!(", "n_("] {
                let mut rest = src.as_str();
                while let Some(at) = rest.find(marker) {
                    // `ptr(` and friends are not markers.
                    let before = rest[..at].chars().next_back();
                    rest = &rest[at + marker.len()..];
                    if before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                        continue;
                    }
                    // rustfmt may put the text on the next line; a call with
                    // anything but a literal is not a marked text.
                    let Some(open) = rest.trim_start().strip_prefix('"') else {
                        continue;
                    };
                    rest = open;
                    let (text, after) = literal(rest)
                        .unwrap_or_else(|| panic!("unterminated text in {}", path.display()));
                    keys.insert(text);
                    rest = after;
                }
            }
        }
        keys
    }

    fn placeholders(s: &str) -> BTreeSet<String> {
        let s = s.replace("{{", "").replace("}}", "");
        let mut found = BTreeSet::new();
        let mut rest = s.as_str();
        while let Some(i) = rest.find('{') {
            let Some(end) = rest[i..].find('}') else {
                break;
            };
            let name = &rest[i + 1..i + end];
            if name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                found.insert(name.to_string());
            }
            rest = &rest[i + end + 1..];
        }
        found
    }

    #[test]
    fn every_language_covers_every_text() {
        let keys = marked();
        assert!(keys.len() > 100, "the source scan found too few texts");
        for lang in LANGS.iter().skip(1) {
            let table: BTreeMap<String, String> =
                serde_json::from_str(lang.table.unwrap()).expect("table is not valid JSON");
            let missing: Vec<_> = keys.iter().filter(|k| !table.contains_key(*k)).collect();
            let stale: Vec<_> = table.keys().filter(|k| !keys.contains(*k)).collect();
            let mismatched: Vec<_> = table
                .iter()
                .filter(|(k, v)| placeholders(k) != placeholders(v))
                .map(|(k, _)| k)
                .collect();
            let empty: Vec<_> = table
                .iter()
                .filter(|(k, v)| v.trim().is_empty() && !k.trim().is_empty())
                .map(|(k, _)| k)
                .collect();
            assert!(
                missing.is_empty() && stale.is_empty() && mismatched.is_empty() && empty.is_empty(),
                "lang/{}.json\nmissing: {missing:#?}\nstale: {stale:#?}\n\
                 placeholders differ: {mismatched:#?}\nempty: {empty:#?}",
                lang.code
            );
        }
    }

    #[test]
    fn codes_are_unique_and_the_source_comes_first() {
        let codes: BTreeSet<_> = LANGS.iter().map(|l| l.code).collect();
        assert_eq!(codes.len(), LANGS.len());
        assert!(LANGS[0].table.is_none());
    }
}
