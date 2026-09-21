# voice-me

Project scaffold in progress — being defined step by step with BMAD-METHOD.

## Running

```bash
cargo run -p voice-me-app
```

The app is tray-resident: closing the Settings window leaves it running in
the tray, and "Settings…" in the tray menu brings the window back.

Pressing the global hotkey summons the prompt overlay: a borderless window
with a single focused field. Type a line and press `Enter` to send it;
`Escape`, or clicking away, closes it and discards what you typed. Dismissing
the overlay never quits the app — it stays in the tray either way.

**Nothing is spoken yet.** A confirmed line is currently only printed to
stdout. Speech generation and playback arrive in later stories.

## Linux: prompt overlay and always-on-top

Whether the overlay is drawn **above a fullscreen window from another
application** depends on the session, the same way the global hotkey's
backend does:

| Session | Overlay window | Above a fullscreen window? |
|---------|----------------|----------------------------|
| X11 | always-on-top, taskbar-less popup | expected — **not yet verified on real hardware** |
| Wayland | ordinary focused window | no — and the compositor, not voice-me, decides where it appears |

This is a platform limitation, not a bug. The GPUI version this app is built
on has no always-on-top window type that works under Wayland, and the one
mechanism that could provide it — `zwlr_layer_shell_v1` — is not implemented
by GNOME/Mutter, so it is deliberately not used. Under Wayland the overlay
still opens, focuses, and accepts typing normally; it may simply sit behind a
fullscreen game instead of over it.

## Linux: global hotkey setup

The global hotkey uses one of two backends, chosen at runtime from the
session:

| Session | Backend | Setup needed |
|---------|---------|--------------|
| X11 | `global-hotkey` (`XGrabKey`) | none |
| Wayland | raw `evdev` read of `/dev/input/event*` | `input`-group membership |

`global-hotkey` is X11-only, and GNOME does not implement the desktop
portal's GlobalShortcuts interface, so on Wayland the hotkey is matched by
reading the keyboard devices directly. That needs read access to the input
devices, which is granted by joining the `input` group — a **one-time**
setup step:

```bash
sudo usermod -aG input $USER
```

**Understand what this grants before running it.** `input`-group membership
lets *any* process running as your user read every keystroke you type,
system-wide — including passwords typed into other applications, on any
desktop or login screen. It is not scoped to voice-me. The Wayland backend
does exactly that: it reads the raw keyboard stream and matches your
combination against it. On X11 nothing of the sort is needed, and voice-me
does not ask for it.

Log out and back in (a new shell is not enough — group membership is
established at login). Until then the Hotkey tab reports the missing access
instead of binding anything; the rest of the app runs normally.

Two properties of the Wayland backend are consequences of the platform, not
bugs:

- The combination **also reaches the focused application** — it is a passive
  read, not an exclusive grab. (The alternative, `EVIOCGRAB`, would steal all
  keyboard input from every other application.)
- **Conflicts cannot be detected.** Nothing is registered with any server, so
  there is no way to learn that another application already uses the
  combination. Inline conflict reporting exists only on X11.

To verify the backend on its own, without the app:

```bash
cargo run -p voice-me-hotkey-linux --example hotkey-spike            # Ctrl+Alt+KeyV
cargo run -p voice-me-hotkey-linux --example hotkey-spike -- Ctrl+Alt+KeyB
```
