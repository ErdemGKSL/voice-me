//! Manual verification for spec-2-1: opens a real tray icon via
//! `LinuxTrayAdapter` and confirms `TrayPort` is wired end to end.
//!
//! This example has no window (tray-only, matching the eventual
//! tray-resident process of Story 2.2), so it must quit explicitly from the
//! tray menu rather than when a (nonexistent) last window closes.
//!
//! Run with: `cargo run -p voice-me-tray-linux --example spike`

use gpui_kit::{App, QuitMode, platform};
use voice_me_core::TrayPort;
use voice_me_tray_linux::LinuxTrayAdapter;

fn main() {
    platform::application()
        .with_quit_mode(QuitMode::Explicit)
        .run(|cx: &mut App| {
            if let Err(error) = LinuxTrayAdapter.show(cx) {
                eprintln!("failed to show tray: {error}");
                cx.quit();
                return;
            }
            println!("voice-me tray spike is running; use the tray menu to quit.");
        });
}
