//! Group `primitives`: the Win32 calls the daemon pays per event, measured
//! in-process against live windows that are probed and never touched.
//! No daemon needed, no input, nothing drawn on screen.

mod enum_windows;
mod gdi;
mod io;
mod topology;
mod window;

use windows_sys::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};

use crate::timing::Runner;

pub fn run(r: &mut Runner) {
    // `get_window_aumid` calls `SHGetPropertyStoreForWindow`, which needs COM
    // initialized on the calling thread; done once for the whole group
    // rather than per benchmark, matching how the daemon initializes it.
    unsafe {
        CoInitializeEx(std::ptr::null_mut(), COINIT_APARTMENTTHREADED as _);
    }

    let targets = window::discover();
    window::bench(r, &targets);
    enum_windows::bench(r);
    topology::bench(r);
    gdi::bench(r);
    io::bench(r);

    unsafe {
        CoUninitialize();
    }
}
