//! Compiles `locales/*.json` into static string tables.
//!
//! `locales/en.json` is the reference: its key set defines `Msg` (simple
//! messages) and `PluralMsg` (keys whose last segment is a CLDR plural
//! category). Every other locale is checked against it here, so a typo in a
//! translation, a placeholder that drifted from English, or a plural group
//! without its `other` form fails the build instead of reaching a user. A
//! *missing* key only warns: the English text is written into the slot so a
//! translation can land incrementally and runtime never sees an empty
//! string.

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::Path;

const PLURAL_CATS: [&str; 6] = ["zero", "one", "two", "few", "many", "other"];
const REFERENCE: &str = "en";

type Table = BTreeMap<String, String>;

fn main() {
    let manifest = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let dir = Path::new(&manifest).join("locales");
    println!("cargo:rerun-if-changed={}", dir.display());

    let mut locales: BTreeMap<String, Table> = BTreeMap::new();
    for entry in fs::read_dir(&dir).expect("locales/ directory is missing") {
        let path = entry.expect("readdir").path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        println!("cargo:rerun-if-changed={}", path.display());
        let tag = path
            .file_stem()
            .and_then(|s| s.to_str())
            .expect("locale file name")
            .to_string();
        assert!(
            !tag.is_empty() && tag.chars().all(|c| c.is_ascii_lowercase()),
            "locale file `{}` must be named by a lowercase language tag",
            path.display()
        );
        let text = fs::read_to_string(&path).expect("read locale");
        let raw: BTreeMap<String, serde_json::Value> = serde_json::from_str(&text)
            .unwrap_or_else(|e| panic!("{}: invalid JSON: {e}", path.display()));
        let mut table = Table::new();
        for (key, value) in raw {
            let s = value
                .as_str()
                .unwrap_or_else(|| panic!("{tag}.json: key `{key}` is not a string"));
            validate_key(&tag, &key);
            validate_braces(&tag, &key, s);
            table.insert(key, s.to_string());
        }
        locales.insert(tag, table);
    }

    let en = locales
        .get(REFERENCE)
        .unwrap_or_else(|| panic!("locales/{REFERENCE}.json is the reference and must exist"));
    let (simple, plural) = partition(en);

    for (tag, table) in &locales {
        if tag == REFERENCE {
            continue;
        }
        check_locale(tag, table, en, &simple, &plural);
    }

    let mut out = String::new();
    emit(&mut out, &locales, en, &simple, &plural);
    let dest = Path::new(&env::var("OUT_DIR").expect("OUT_DIR")).join("i18n_gen.rs");
    fs::write(&dest, out).expect("write i18n_gen.rs");
}

/// `surface.area.item[.part]`: lowercase segments joined by `.`, `_` allowed
/// inside a segment.
fn validate_key(tag: &str, key: &str) {
    let ok = !key.is_empty()
        && key.split('.').all(|seg| {
            !seg.is_empty()
                && seg.starts_with(|c: char| c.is_ascii_lowercase())
                && seg
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        });
    assert!(ok, "{tag}.json: key `{key}` is not `lower.snake.segments`");
}

/// Braces are either doubled (literal) or wrap an identifier placeholder.
fn validate_braces(tag: &str, key: &str, s: &str) {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => {
                if bytes.get(i + 1) == Some(&b'{') {
                    i += 2;
                    continue;
                }
                let end = s[i + 1..]
                    .find('}')
                    .unwrap_or_else(|| panic!("{tag}.json: `{key}` has an unclosed `{{`"));
                let name = &s[i + 1..i + 1 + end];
                assert!(
                    !name.is_empty()
                        && name
                            .chars()
                            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
                    "{tag}.json: `{key}` has an invalid placeholder `{{{name}}}`"
                );
                i += end + 2;
            }
            b'}' => {
                assert!(
                    bytes.get(i + 1) == Some(&b'}'),
                    "{tag}.json: `{key}` has a stray `}}`"
                );
                i += 2;
            }
            _ => i += 1,
        }
    }
}

fn placeholders(s: &str) -> BTreeSet<String> {
    let mut set = BTreeSet::new();
    let mut rest = s;
    while let Some(start) = rest.find('{') {
        if rest[start..].starts_with("{{") {
            rest = &rest[start + 2..];
            continue;
        }
        let after = &rest[start + 1..];
        let end = after.find('}').expect("validated");
        set.insert(after[..end].to_string());
        rest = &after[end + 1..];
    }
    set
}

/// Splits `base.category` into `(base, category index)` when the last segment
/// is a plural category.
fn plural_split(key: &str) -> Option<(&str, usize)> {
    let (base, last) = key.rsplit_once('.')?;
    let idx = PLURAL_CATS.iter().position(|c| *c == last)?;
    Some((base, idx))
}

/// Simple keys (sorted) and plural groups (`base -> [Option<text>; 6]`).
fn partition(en: &Table) -> (Vec<String>, BTreeMap<String, [Option<String>; 6]>) {
    let mut simple = Vec::new();
    let mut plural: BTreeMap<String, [Option<String>; 6]> = BTreeMap::new();
    for (key, text) in en {
        match plural_split(key) {
            Some((base, idx)) => {
                plural.entry(base.to_string()).or_default()[idx] = Some(text.clone());
            }
            None => simple.push(key.clone()),
        }
    }
    for (base, forms) in &plural {
        assert!(
            forms[5].is_some(),
            "{REFERENCE}.json: plural `{base}` needs an `.other` form"
        );
        assert!(
            !en.contains_key(base),
            "{REFERENCE}.json: `{base}` is both a plain key and a plural group"
        );
    }
    (simple, plural)
}

fn check_locale(
    tag: &str,
    table: &Table,
    en: &Table,
    simple: &[String],
    plural: &BTreeMap<String, [Option<String>; 6]>,
) {
    for (key, text) in table {
        match plural_split(key) {
            Some((base, _)) if plural.contains_key(base) => {
                let reference = plural[base][5].as_deref().expect("other exists");
                assert!(
                    placeholders(text) == placeholders(reference),
                    "{tag}.json: placeholders of `{key}` differ from {REFERENCE}.json"
                );
            }
            _ => {
                let reference = en.get(key).unwrap_or_else(|| {
                    panic!("{tag}.json: unknown key `{key}` (not in {REFERENCE}.json)")
                });
                assert!(
                    placeholders(text) == placeholders(reference),
                    "{tag}.json: placeholders of `{key}` differ from {REFERENCE}.json"
                );
            }
        }
    }
    for key in simple {
        if !table.contains_key(key) {
            println!("cargo:warning={tag}.json: missing `{key}` (falls back to English)");
        }
    }
    for base in plural.keys() {
        let has_any = table
            .keys()
            .any(|k| plural_split(k).is_some_and(|(b, _)| b == base));
        let has_other = table.contains_key(&format!("{base}.other"));
        assert!(
            !has_any || has_other,
            "{tag}.json: plural `{base}` needs an `.other` form"
        );
        if !has_any {
            println!("cargo:warning={tag}.json: missing plural `{base}` (falls back to English)");
        }
    }
}

/// `settings.system.show_all.title` -> `SettingsSystemShowAllTitle`.
fn variant_name(key: &str, seen: &mut BTreeSet<String>) -> String {
    let mut name = String::new();
    for seg in key.split(['.', '_']) {
        let mut chars = seg.chars();
        if let Some(first) = chars.next() {
            name.push(first.to_ascii_uppercase());
            name.extend(chars);
        }
    }
    if name.starts_with(|c: char| c.is_ascii_digit()) {
        name.insert(0, 'K');
    }
    assert!(
        seen.insert(name.clone()),
        "key `{key}` collides with another key's variant name `{name}`"
    );
    name
}

fn pascal(tag: &str) -> String {
    let mut chars = tag.chars();
    match chars.next() {
        Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
        None => String::new(),
    }
}

fn lit(s: &str) -> String {
    format!("{s:?}")
}

fn opt_lit(s: Option<&str>) -> String {
    match s {
        Some(s) => format!("Some({})", lit(s)),
        None => "None".to_string(),
    }
}

fn emit(
    out: &mut String,
    locales: &BTreeMap<String, Table>,
    en: &Table,
    simple: &[String],
    plural: &BTreeMap<String, [Option<String>; 6]>,
) {
    // Reference first, then the others alphabetically: `Lang::En` is 0 so the
    // atomic's zero-initialised default is English.
    let mut tags: Vec<&String> = vec![locales.keys().find(|t| *t == REFERENCE).expect("en")];
    tags.extend(locales.keys().filter(|t| *t != REFERENCE));

    out.push_str("// Generated by build.rs from locales/*.json. Do not edit.\n\n");

    // Lang
    out.push_str("#[repr(u8)]\n#[derive(Clone, Copy, PartialEq, Eq, Debug)]\npub enum Lang {\n");
    for (i, tag) in tags.iter().enumerate() {
        out.push_str(&format!("    {} = {i},\n", pascal(tag)));
    }
    out.push_str("}\n\nimpl Lang {\n");
    out.push_str(&format!(
        "    pub const ALL: [Lang; {}] = [{}];\n",
        tags.len(),
        tags.iter()
            .map(|t| format!("Lang::{}", pascal(t)))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    out.push_str("    pub const fn tag(self) -> &'static str {\n        match self {\n");
    for tag in &tags {
        out.push_str(&format!(
            "            Lang::{} => {},\n",
            pascal(tag),
            lit(tag)
        ));
    }
    out.push_str("        }\n    }\n");
    out.push_str("    pub(crate) const fn from_u8(v: u8) -> Lang {\n        match v {\n");
    for (i, tag) in tags.iter().enumerate().skip(1) {
        out.push_str(&format!("            {i} => Lang::{},\n", pascal(tag)));
    }
    out.push_str(&format!(
        "            _ => Lang::{},\n        }}\n    }}\n}}\n\n",
        pascal(tags[0])
    ));

    // Msg
    let mut seen = BTreeSet::new();
    let simple_variants: Vec<String> = simple.iter().map(|k| variant_name(k, &mut seen)).collect();
    out.push_str("#[repr(u16)]\n#[derive(Clone, Copy, PartialEq, Eq, Debug)]\npub enum Msg {\n");
    for (i, v) in simple_variants.iter().enumerate() {
        out.push_str(&format!("    {v} = {i},\n"));
    }
    out.push_str(&format!(
        "}}\npub const MSG_COUNT: usize = {};\n\n",
        simple.len()
    ));

    // PluralMsg
    let mut seen_plural = BTreeSet::new();
    let plural_bases: Vec<&String> = plural.keys().collect();
    let plural_variants: Vec<String> = plural_bases
        .iter()
        .map(|k| variant_name(k, &mut seen_plural))
        .collect();
    out.push_str(
        "#[repr(u16)]\n#[derive(Clone, Copy, PartialEq, Eq, Debug)]\npub enum PluralMsg {\n",
    );
    for (i, v) in plural_variants.iter().enumerate() {
        out.push_str(&format!("    {v} = {i},\n"));
    }
    out.push_str(&format!(
        "}}\npub const PLURAL_COUNT: usize = {};\n\n",
        plural_bases.len()
    ));

    // Keys (for tests / diagnostics).
    out.push_str("#[allow(dead_code)]\npub(crate) static MSG_KEYS: [&str; MSG_COUNT] = [\n");
    for k in simple {
        out.push_str(&format!("    {},\n", lit(k)));
    }
    out.push_str("];\n\n");

    // Tables
    for tag in &tags {
        let table = &locales[*tag];
        let upper = tag.to_ascii_uppercase();
        out.push_str(&format!("static TABLE_{upper}: [&str; MSG_COUNT] = [\n"));
        for k in simple {
            let text = table
                .get(k)
                .or_else(|| en.get(k))
                .expect("reference has key");
            out.push_str(&format!("    {},\n", lit(text)));
        }
        out.push_str("];\n");
        out.push_str(&format!(
            "static PLURAL_{upper}: [[Option<&str>; 6]; PLURAL_COUNT] = [\n"
        ));
        for base in &plural_bases {
            let en_forms = &plural[*base];
            let has_any = table
                .keys()
                .any(|k| plural_split(k).is_some_and(|(b, _)| b == base.as_str()));
            out.push_str("    [");
            for (i, cat) in PLURAL_CATS.iter().enumerate() {
                let text = if has_any {
                    table.get(&format!("{base}.{cat}")).map(String::as_str)
                } else {
                    en_forms[i].as_deref()
                };
                out.push_str(&opt_lit(text));
                out.push_str(", ");
            }
            out.push_str("],\n");
        }
        out.push_str("];\n\n");
    }

    out.push_str(&format!(
        "pub(crate) static TABLES: [&[&str; MSG_COUNT]; {}] = [{}];\n",
        tags.len(),
        tags.iter()
            .map(|t| format!("&TABLE_{}", t.to_ascii_uppercase()))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    out.push_str(&format!(
        "pub(crate) static PLURALS: [&[[Option<&str>; 6]; PLURAL_COUNT]; {}] = [{}];\n\n",
        tags.len(),
        tags.iter()
            .map(|t| format!("&PLURAL_{}", t.to_ascii_uppercase()))
            .collect::<Vec<_>>()
            .join(", ")
    ));

    // Pseudo-locale: debug builds only.
    out.push_str("#[cfg(debug_assertions)]\npub(crate) static TABLE_XX: [&str; MSG_COUNT] = [\n");
    for k in simple {
        out.push_str(&format!("    {},\n", lit(&pseudo(&en[k]))));
    }
    out.push_str("];\n");
    out.push_str(
        "#[cfg(debug_assertions)]\npub(crate) static PLURAL_XX: [[Option<&str>; 6]; PLURAL_COUNT] = [\n",
    );
    for base in &plural_bases {
        out.push_str("    [");
        for form in &plural[*base] {
            out.push_str(&opt_lit(form.as_deref().map(pseudo).as_deref()));
            out.push_str(", ");
        }
        out.push_str("],\n");
    }
    out.push_str("];\n");
}

/// `[Ẃéĺćómé {n}~~~]`: accented vowels expose hard-coded fonts, the padding
/// (to 140 % of the English length) exposes clipped layouts, the brackets
/// expose strings that never went through the table. Placeholders survive.
fn pseudo(s: &str) -> String {
    let mut out = String::from("[");
    let mut rest = s;
    while !rest.is_empty() {
        if let Some(start) = rest.find('{') {
            for c in rest[..start].chars() {
                out.push(accent(c));
            }
            let after = &rest[start..];
            let end = after.find('}').map(|e| e + 1).unwrap_or(after.len());
            out.push_str(&after[..end]);
            rest = &after[end..];
        } else {
            for c in rest.chars() {
                out.push(accent(c));
            }
            rest = "";
        }
    }
    let target = (s.chars().count() * 14).div_ceil(10);
    let current = s.chars().count();
    for _ in current..target {
        out.push('~');
    }
    out.push(']');
    out
}

fn accent(c: char) -> char {
    match c {
        'a' => 'á',
        'e' => 'é',
        'i' => 'í',
        'o' => 'ó',
        'u' => 'ú',
        'A' => 'Á',
        'E' => 'É',
        'I' => 'Í',
        'O' => 'Ó',
        'U' => 'Ú',
        'n' => 'ñ',
        _ => c,
    }
}
