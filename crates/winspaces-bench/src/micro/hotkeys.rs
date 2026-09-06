//! `hotkeys/decode`: mapping a `WM_HOTKEY` id back to the action it names —
//! paid on every hotkey press.

use winspaces_core::hotkeys::{
    decode_hotkey, HOTKEY_ID_MISSION_CONTROL, HOTKEY_ID_MOVE_BASE, HOTKEY_ID_SWITCH_BASE,
    HOTKEY_ID_TILING_FOCUS_LEFT, HOTKEY_ID_TILING_TOGGLE,
};

use crate::timing::Runner;

pub fn bench(r: &mut Runner) {
    let ids = [
        HOTKEY_ID_SWITCH_BASE,
        HOTKEY_ID_MOVE_BASE + 2,
        HOTKEY_ID_MISSION_CONTROL,
        HOTKEY_ID_TILING_TOGGLE,
        HOTKEY_ID_TILING_FOCUS_LEFT,
    ];
    r.bench("hotkeys/decode", || {
        for &id in &ids {
            std::hint::black_box(decode_hotkey(std::hint::black_box(id)));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_fixture_id_decodes_to_something() {
        let ids = [
            HOTKEY_ID_SWITCH_BASE,
            HOTKEY_ID_MOVE_BASE + 2,
            HOTKEY_ID_MISSION_CONTROL,
            HOTKEY_ID_TILING_TOGGLE,
            HOTKEY_ID_TILING_FOCUS_LEFT,
        ];
        for id in ids {
            assert!(decode_hotkey(id).is_some(), "id {id} did not decode");
        }
    }

    #[test]
    fn a_smoke_run_covers_the_named_benchmark() {
        let mut r = Runner::new(true, None, false);
        bench(&mut r);
        assert!(r.entries.iter().any(|e| e.name == "hotkeys/decode"));
    }
}
