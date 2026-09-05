//! Display form of a hotkey in the UI language (`Ctrl+Alt+1`,
//! `Ctrl+Mayús+Supr`). Display only: the config persists `modifiers`/`vk`
//! numerically and nothing parses this back.

use crate::config::Hotkey;

/// Display form of a hotkey in the current UI language: `Ctrl+Alt+1`,
/// `Ctrl+Mayús+Supr`. Display only — config persists `modifiers`/`vk`
/// numerically and nothing parses this back.
pub fn hotkey_to_string(hk: &Hotkey) -> String {
    hotkey_to_string_in(crate::i18n::current(), hk)
}

/// `Msg` for the keys that have a name rather than a legend. Letters,
/// digits, F-keys and punctuation are the same on every keyboard and stay
/// untranslated.
fn key_msg(vk: u32) -> Option<crate::i18n::Msg> {
    use crate::i18n::Msg;
    Some(match vk {
        0x09 => Msg::KeyTab,
        0x1B => Msg::KeyEsc,
        0x20 => Msg::KeySpace,
        0x0D => Msg::KeyEnter,
        0x08 => Msg::KeyBackspace,
        0x2E => Msg::KeyDelete,
        0x24 => Msg::KeyHome,
        0x23 => Msg::KeyEnd,
        0x21 => Msg::KeyPageUp,
        0x22 => Msg::KeyPageDown,
        0x25 => Msg::KeyLeft,
        0x27 => Msg::KeyRight,
        0x26 => Msg::KeyUp,
        0x28 => Msg::KeyDown,
        _ => return None,
    })
}

pub fn hotkey_to_string_in(lang: crate::i18n::Lang, hk: &Hotkey) -> String {
    use crate::i18n::{t_in, Msg};
    if hk.vk == 0 {
        return t_in(lang, Msg::KeyUnassigned).to_string();
    }
    let mut parts: Vec<&str> = Vec::with_capacity(5);
    if (hk.modifiers & 0x0002) != 0 {
        parts.push(t_in(lang, Msg::KeyCtrl));
    }
    if (hk.modifiers & 0x0001) != 0 {
        parts.push(t_in(lang, Msg::KeyAlt));
    }
    if (hk.modifiers & 0x0004) != 0 {
        parts.push(t_in(lang, Msg::KeyShift));
    }
    if (hk.modifiers & 0x0008) != 0 {
        parts.push(t_in(lang, Msg::KeyWin));
    }

    let vk = hk.vk;
    let vk_str: String = if (0x41..=0x5A).contains(&vk) || (0x30..=0x39).contains(&vk) {
        char::from_u32(vk)
            .map(|c| c.to_string())
            .unwrap_or_default()
    } else if (0x70..=0x87).contains(&vk) {
        format!("F{}", vk - 0x70 + 1)
    } else if let Some(msg) = key_msg(vk) {
        t_in(lang, msg).to_string()
    } else {
        match vk {
            0xBB => "+".to_string(),
            0xBD => "-".to_string(),
            0xBC => ",".to_string(),
            0xBE => ".".to_string(),
            0xBA => ";".to_string(),
            0xBF => "/".to_string(),
            0xC0 => "`".to_string(),
            0xDB => "[".to_string(),
            0xDD => "]".to_string(),
            0xDC => "\\".to_string(),
            0xDE => "'".to_string(),
            _ => format!("VK{}", vk),
        }
    };
    parts.push(&vk_str);
    parts.join("+")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hotkey_to_string_formats_known_keys() {
        let hk = Hotkey {
            modifiers: 0x0002 | 0x0001,
            vk: 0x31,
        };
        assert_eq!(hotkey_to_string(&hk), "Ctrl+Alt+1");
        let none = Hotkey::default();
        assert_eq!(hotkey_to_string(&none), "Unassigned");
        let f5 = Hotkey {
            modifiers: 0x0008,
            vk: 0x74,
        };
        assert_eq!(hotkey_to_string(&f5), "Win+F5");
    }

    #[test]
    fn hotkey_to_string_in_uses_the_given_language() {
        use crate::i18n::Lang;
        let hk = Hotkey {
            modifiers: 0x0002 | 0x0004,
            vk: 0x2E,
        };
        assert_eq!(hotkey_to_string_in(Lang::En, &hk), "Ctrl+Shift+Delete");
        assert_eq!(hotkey_to_string_in(Lang::Es, &hk), "Ctrl+Mayús+Supr");
        assert_eq!(
            hotkey_to_string_in(Lang::Es, &Hotkey::default()),
            "Sin asignar"
        );
    }
}
