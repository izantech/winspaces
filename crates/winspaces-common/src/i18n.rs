//! Compile-time embedded UI strings.
//!
//! `locales/en.json` defines the key set; `build.rs` generates `Lang`, `Msg`,
//! `PluralMsg` and one static table per language. The current language is a
//! process-wide atomic (both the daemon and the settings process are
//! single-UI-thread, and the hook callbacks run on that thread too), so a
//! lookup is two array indexes. The pure `*_in(lang, …)` variants exist so
//! tests never touch the global and the exact-string tests elsewhere in the
//! workspace stay valid under any test ordering: nothing but
//! `current_roundtrip` below ever calls `set_current` from a test.
//!
//! What is deliberately *not* here: log lines, window class names, IPC
//! titles, registry keys, the scheduled-task name, JSON keys and the tray
//! tooltip brand name — identifiers, not text.

use std::sync::atomic::{AtomicU8, Ordering};

mod format;
#[allow(clippy::enum_variant_names)]
mod generated {
    include!(concat!(env!("OUT_DIR"), "/i18n_gen.rs"));
}

pub use format::Arg;
pub use generated::{Lang, Msg, PluralMsg};
use generated::{PLURALS, TABLES};

/// The `Config.language` value that means "follow the Windows display
/// language".
pub const SYSTEM_TAG: &str = "system";

/// `Lang::En` is discriminant 0 by construction (build.rs emits the
/// reference locale first), so the zero-initialised default is English.
static CURRENT: AtomicU8 = AtomicU8::new(0);

pub fn current() -> Lang {
    Lang::from_u8(CURRENT.load(Ordering::Relaxed))
}

pub fn set_current(lang: Lang) {
    CURRENT.store(lang as u8, Ordering::Relaxed);
}

/// A message without arguments, in the current language. Never allocates.
#[inline]
pub fn t(msg: Msg) -> &'static str {
    t_in(current(), msg)
}

#[inline]
pub fn t_in(lang: Lang, msg: Msg) -> &'static str {
    #[cfg(debug_assertions)]
    if format::pseudo_enabled() {
        return generated::TABLE_XX[msg as usize];
    }
    TABLES[lang as usize][msg as usize]
}

/// A message with `{name}` arguments. Prefer the `tr!` macro at call sites.
pub fn tf(msg: Msg, args: &[Arg]) -> String {
    tf_in(current(), msg, args)
}

pub fn tf_in(lang: Lang, msg: Msg, args: &[Arg]) -> String {
    format::substitute(t_in(lang, msg), args)
}

/// A plural message: the CLDR category for `n` in `lang` picks the form,
/// falling back to `other`, then to English `other`. `n` is not injected
/// automatically — pass it in `args` under the name the template uses.
pub fn tn(msg: PluralMsg, n: u64, args: &[Arg]) -> String {
    tn_in(current(), msg, n, args)
}

pub fn tn_in(lang: Lang, msg: PluralMsg, n: u64, args: &[Arg]) -> String {
    let category = plural_category(lang, n) as usize;
    let forms = plural_forms(lang, msg);
    let template = forms[category]
        .or(forms[Plural::Other as usize])
        .or(PLURALS[Lang::En as usize][msg as usize][Plural::Other as usize])
        .unwrap_or_default();
    format::substitute(template, args)
}

fn plural_forms(lang: Lang, msg: PluralMsg) -> &'static [Option<&'static str>; 6] {
    #[cfg(debug_assertions)]
    if format::pseudo_enabled() {
        return &generated::PLURAL_XX[msg as usize];
    }
    &PLURALS[lang as usize][msg as usize]
}

/// CLDR plural categories, in the order the generated tables store them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(usize)]
pub enum Plural {
    Zero = 0,
    One = 1,
    Two = 2,
    Few = 3,
    Many = 4,
    Other = 5,
}

/// CLDR integer plural rules. Exhaustive on `Lang`: adding a locale file
/// does not compile until its rule is written here.
pub fn plural_category(lang: Lang, n: u64) -> Plural {
    match lang {
        Lang::En | Lang::Es => {
            if n == 1 {
                Plural::One
            } else {
                Plural::Other
            }
        }
    }
}

impl Lang {
    /// `"es"`, `"es-ES"`, `"ES_MX"` → `Es`; unknown → `None`.
    pub fn from_tag(tag: &str) -> Option<Lang> {
        let tag = tag.trim().to_ascii_lowercase();
        let primary = tag.split(['-', '_']).next().unwrap_or("");
        Lang::ALL.iter().copied().find(|l| l.tag() == primary)
    }

    /// The language's own name for itself — the combo label, deliberately
    /// not translated into the current language.
    pub fn label(self) -> &'static str {
        t_in(self, Msg::LangName)
    }

    /// Primary language of a Win32 `LANGID` (low 10 bits). Pure so it is
    /// testable without a Windows locale.
    pub fn from_langid(langid: u16) -> Option<Lang> {
        match langid & 0x3FF {
            0x09 => Some(Lang::En),
            0x0A => Some(Lang::Es),
            _ => None,
        }
    }

    /// The Windows display language, or English when it is not shipped.
    pub fn from_windows_ui_language() -> Lang {
        use windows_sys::Win32::Globalization::GetUserDefaultUILanguage;
        Lang::from_langid(unsafe { GetUserDefaultUILanguage() }).unwrap_or(Lang::En)
    }

    /// Resolve a `Config.language` value: `"system"` (or anything
    /// unrecognised) follows Windows, a known tag pins the language.
    pub fn resolve(setting: &str) -> Lang {
        if setting.eq_ignore_ascii_case(SYSTEM_TAG) {
            return Lang::from_windows_ui_language();
        }
        Lang::from_tag(setting).unwrap_or_else(Lang::from_windows_ui_language)
    }
}

/// `tr!(Msg::X)` → `&'static str`; `tr!(Msg::X, n = i + 1, path = p)` →
/// `String` with the named placeholders substituted.
#[macro_export]
macro_rules! tr {
    ($msg:expr) => {
        $crate::i18n::t($msg)
    };
    ($msg:expr, $($name:ident = $val:expr),+ $(,)?) => {
        $crate::i18n::tf($msg, &[$((stringify!($name), &$val as &dyn ::std::fmt::Display)),+])
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_is_the_zero_default() {
        assert_eq!(Lang::En as u8, 0);
        assert_eq!(Lang::ALL[0], Lang::En);
    }

    #[test]
    fn every_lang_has_a_self_name() {
        for lang in Lang::ALL {
            assert!(!lang.label().is_empty(), "{:?} has no lang.name", lang);
        }
        assert_eq!(Lang::En.label(), "English");
    }

    #[test]
    fn simple_lookup_is_static_english_by_default() {
        assert_eq!(t_in(Lang::En, Msg::TrayMissionControl), "Mission Control");
    }

    #[test]
    fn formatted_lookup_substitutes() {
        assert_eq!(tf_in(Lang::En, Msg::TraySpace, &[("n", &3)]), "Space 3");
        assert_eq!(
            tf_in(
                Lang::En,
                Msg::BannerAutosaveMessage,
                &[("reason", &"Saved")]
            ),
            "Saved. WinSpaces daemon reloaded live via Win32 IPC."
        );
    }

    #[test]
    fn plural_en_es_one_vs_other() {
        for lang in [Lang::En, Lang::Es] {
            assert_eq!(plural_category(lang, 0), Plural::Other);
            assert_eq!(plural_category(lang, 1), Plural::One);
            assert_eq!(plural_category(lang, 2), Plural::Other);
            assert_eq!(plural_category(lang, 21), Plural::Other);
        }
        assert_eq!(
            tn_in(Lang::En, PluralMsg::McWindowCount, 1, &[("n", &1)]),
            "1 window"
        );
        assert_eq!(
            tn_in(Lang::En, PluralMsg::McWindowCount, 4, &[("n", &4)]),
            "4 windows"
        );
    }

    #[test]
    fn from_tag_accepts_region_suffix_and_case() {
        assert_eq!(Lang::from_tag("es-ES"), Some(Lang::Es));
        assert_eq!(Lang::from_tag("ES_MX"), Some(Lang::Es));
        assert_eq!(Lang::from_tag(" en "), Some(Lang::En));
        assert_eq!(Lang::from_tag("pt"), None);
        assert_eq!(Lang::from_tag(""), None);
    }

    #[test]
    fn from_langid_uses_the_primary_language() {
        assert_eq!(Lang::from_langid(0x0C0A), Some(Lang::Es)); // es-ES
        assert_eq!(Lang::from_langid(0x080A), Some(Lang::Es)); // es-MX
        assert_eq!(Lang::from_langid(0x0409), Some(Lang::En)); // en-US
        assert_eq!(Lang::from_langid(0x0809), Some(Lang::En)); // en-GB
        assert_eq!(Lang::from_langid(0x040C), None); // fr-FR
    }

    /// The only test that touches the global; it restores English before
    /// returning so every other exact-string test in the workspace keeps
    /// reading the reference locale.
    #[test]
    fn current_roundtrip() {
        set_current(Lang::Es);
        assert_eq!(current(), Lang::Es);
        set_current(Lang::En);
        assert_eq!(current(), Lang::En);
    }

    #[test]
    fn tr_macro_forms() {
        let s: &'static str = tr!(Msg::TrayNewSpace);
        assert_eq!(s, "New Space");
        let n = 7;
        assert_eq!(tr!(Msg::TrayDisplay, n = n), "Display 7");
    }
}
