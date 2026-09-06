//! `hotkey/to_string`: the settings and menu label rendered for a configured
//! hotkey.

use winspaces_common::hotkey_label::hotkey_to_string_in;
use winspaces_common::i18n::Lang;
use winspaces_common::Hotkey;

use crate::timing::Runner;

pub fn bench(r: &mut Runner) {
    let hk = Hotkey {
        modifiers: 0x0001 | 0x0002 | 0x0004, // Alt + Ctrl + Shift
        vk: 0x2E,                            // Delete
    };
    r.bench("hotkey/to_string", || {
        std::hint::black_box(hotkey_to_string_in(Lang::En, &hk));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctrl_alt_shift_delete_formats_as_expected() {
        let hk = Hotkey {
            modifiers: 0x0001 | 0x0002 | 0x0004,
            vk: 0x2E,
        };
        assert_eq!(hotkey_to_string_in(Lang::En, &hk), "Ctrl+Alt+Shift+Delete");
    }

    #[test]
    fn a_smoke_run_covers_the_named_benchmark() {
        let mut r = Runner::new(true, None, false);
        bench(&mut r);
        assert!(r.entries.iter().any(|e| e.name == "hotkey/to_string"));
    }
}
