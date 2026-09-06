//! `topology/signature`: the per-reconcile fingerprint of the attached
//! monitor set.

use winspaces_core::topology::signature_from_ids;

use crate::timing::Runner;

fn fixture_ids() -> Vec<String> {
    vec![
        "\\\\?\\DISPLAY#BNQ805B#5&1f33c64f&0&UID4354#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}"
            .to_string(),
        "\\\\?\\DISPLAY#SAM0E4C#4&2a3b4c5d&0&UID256#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}"
            .to_string(),
        "\\\\?\\DISPLAY#AUO2100#5&3b4c5d6e&0&UID512#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}"
            .to_string(),
    ]
}

pub fn bench(r: &mut Runner) {
    let ids = fixture_ids();
    r.bench("topology/signature/3", || {
        std::hint::black_box(signature_from_ids(&ids));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_smoke_run_covers_the_named_benchmark() {
        let mut r = Runner::new(true, None, false);
        bench(&mut r);
        assert!(r.entries.iter().any(|e| e.name == "topology/signature/3"));
    }
}
