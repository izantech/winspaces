//! `{name}` substitution and the debug-only pseudo-locale switch.

use std::fmt::{Display, Write};

/// One named argument for `tf`/`tn`.
pub type Arg<'a> = (&'a str, &'a dyn Display);

/// Substitute `{name}` placeholders from `args`; `{{` and `}}` are literal
/// braces. build.rs has already verified every translation carries the same
/// placeholder set as English, so an unknown placeholder here is a caller
/// bug: it is left in the output verbatim (and trips a `debug_assert!`),
/// never dropped, so the mistake is visible on screen.
pub fn substitute(template: &str, args: &[Arg]) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut rest = template;
    while let Some(start) = rest.find(['{', '}']) {
        out.push_str(&rest[..start]);
        let after = &rest[start..];
        if let Some(tail) = after.strip_prefix("{{") {
            out.push('{');
            rest = tail;
        } else if let Some(tail) = after.strip_prefix("}}") {
            out.push('}');
            rest = tail;
        } else if let Some(end) = after.find('}') {
            let name = &after[1..end];
            match args.iter().find(|(k, _)| *k == name) {
                Some((_, value)) => {
                    let _ = write!(out, "{value}");
                }
                None => {
                    debug_assert!(false, "unknown placeholder {{{name}}} in `{template}`");
                    out.push_str(&after[..=end]);
                }
            }
            rest = &after[end + 1..];
        } else {
            out.push_str(after);
            rest = "";
        }
    }
    out.push_str(rest);
    out
}

/// `WINSPACES_PSEUDOLOC=1` swaps every lookup for the bracketed, accented,
/// padded pseudo-locale generated from English. Debug builds only; the
/// release binary carries neither the table nor this check.
#[cfg(debug_assertions)]
pub fn pseudo_enabled() -> bool {
    use std::sync::OnceLock;
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("WINSPACES_PSEUDOLOC")
            .is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitutes_named_args_in_any_order() {
        let out = substitute("{b} then {a}", &[("a", &1), ("b", &"two")]);
        assert_eq!(out, "two then 1");
    }

    #[test]
    fn doubled_braces_are_literal() {
        let out = substitute("{{n}} = {n}", &[("n", &3)]);
        assert_eq!(out, "{n} = 3");
    }

    #[test]
    fn no_placeholders_is_identity() {
        assert_eq!(substitute("plain text", &[]), "plain text");
    }

    #[test]
    #[cfg_attr(debug_assertions, should_panic(expected = "unknown placeholder"))]
    fn unknown_placeholder_is_kept_verbatim() {
        let out = substitute("hello {who}", &[]);
        assert_eq!(out, "hello {who}");
    }
}
