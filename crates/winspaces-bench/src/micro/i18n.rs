//! `i18n/*`: string lookup, placeholder substitution and plural selection —
//! every painted string pays one of these.

use winspaces_common::i18n::{t_in, tf_in, tn_in, Lang, Msg, PluralMsg};

use crate::timing::Runner;

// Inputs go through `black_box` too: a lookup whose language and key are
// compile-time constants folds to a single load and reads as 0 ns.
pub fn bench(r: &mut Runner) {
    r.bench("i18n/t_in", || {
        std::hint::black_box(t_in(
            std::hint::black_box(Lang::En),
            std::hint::black_box(Msg::SettingsDelete),
        ));
    });

    // Two placeholders, and a non-default language table.
    r.bench("i18n/tf_in/2args", || {
        std::hint::black_box(tf_in(
            std::hint::black_box(Lang::Es),
            std::hint::black_box(Msg::BannerExportFailedMessage),
            &[
                ("file", &"settings.json"),
                ("error", &"unexpected end of file"),
            ],
        ));
    });

    r.bench("i18n/tn_in", || {
        std::hint::black_box(tn_in(
            std::hint::black_box(Lang::En),
            std::hint::black_box(PluralMsg::McWindowCount),
            std::hint::black_box(3),
            &[("n", &3)],
        ));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_smoke_run_covers_every_named_benchmark() {
        let mut r = Runner::new(true, None, false);
        bench(&mut r);
        let names: Vec<&str> = r.entries.iter().map(|e| e.name.as_str()).collect();
        for expected in ["i18n/t_in", "i18n/tf_in/2args", "i18n/tn_in"] {
            assert!(names.contains(&expected), "missing benchmark {expected}");
        }
    }
}
