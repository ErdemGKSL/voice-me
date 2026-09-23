//! Manual verification for spec-2-3: binds a real global hotkey through
//! `LinuxHotkeyAdapter` and prints one line per press, confirming
//! `HotkeyPort` is wired end to end on whichever backend this session calls
//! for.
//!
//! No window and no tray — just the adapter and the shared `AppEvent`
//! channel, mirroring `voice-me-tray-linux/examples/spike.rs`. Named
//! `hotkey-spike` rather than `spike` because cargo writes every example in
//! the workspace to one shared `target/debug/examples/` directory, so two
//! examples called `spike` would overwrite each other's binary.
//!
//! Run with: `cargo run -p voice-me-hotkey-linux --example hotkey-spike`
//! Optionally pass a combination: `… --example hotkey-spike -- Ctrl+Alt+KeyB`
//!
//! On Wayland this needs read access to `/dev/input/event*`
//! (`sudo usermod -aG input $USER`, then log back in); without it the run
//! must report the missing `input`-group membership rather than hang or
//! silently do nothing.

// The library body is `#![cfg(target_os = "linux")]`, so on any other
// target it compiles to an empty crate and these imports would not resolve
// — and `cargo test --workspace` on Windows builds this example.

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("the Linux hotkey spike is Linux-only");
}

#[cfg(target_os = "linux")]
use futures::StreamExt as _;
#[cfg(target_os = "linux")]
use futures::channel::mpsc;
#[cfg(target_os = "linux")]
use voice_me_core::{AppEvent, HotkeyPort as _};
#[cfg(target_os = "linux")]
use voice_me_hotkey_linux::{LinuxHotkeyAdapter, SessionKind, session_kind};

#[cfg(target_os = "linux")]
fn main() {
    let hotkey = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "Ctrl+Alt+KeyV".to_string());

    let (events, mut receiver) = mpsc::unbounded::<AppEvent>();
    let adapter = LinuxHotkeyAdapter::new(events.clone());

    let backend = match session_kind() {
        SessionKind::X11 => "X11 (global-hotkey / XGrabKey)",
        SessionKind::Wayland => "Wayland (evdev)",
    };
    println!("session backend: {backend}");

    if let Err(error) = adapter.start_listening(&hotkey, events) {
        eprintln!("failed to bind {hotkey}: {error}");
        std::process::exit(1);
    }

    println!("bound {hotkey} — press it (Ctrl+C to quit).");

    futures::executor::block_on(async move {
        let mut presses = 0_u32;
        while let Some(event) = receiver.next().await {
            if matches!(event, AppEvent::HotkeyPressed) {
                presses += 1;
                println!("hotkey pressed ({presses})");
            }
        }
    });
}
