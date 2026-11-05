---
title: 'Windows Virtual Microphone via VB-CABLE, installed with one click'
type: 'feature'
created: '2026-09-24'
status: 'done'
route: 'dispatch'
review_loop_iteration: 0
baseline_commit: 'aedc7a52c82845ea0ed7853c63bf79d5389ff1ce'
context: ['{project-root}/_bmad-output/implementation-artifacts/epic-2-context.md', '{project-root}/_bmad-output/implementation-artifacts/spec-2-9-play-generated-audio-through-the-virtual-microphone.md']
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** On Windows, every Speak Action ends in "the Windows virtual microphone is not implemented yet (Story 2.8)". The planned driver cannot fix that. VirtualDrivers/Virtual-Audio-Driver 25.7.14 fills its virtual mic's buffer with silence (`RtlZeroMemory` in `minwavertstream.cpp`, checked at the tag). What is played to its speaker only goes to a file. Speaker-to-mic routing is a paid custom build. This answers PRD Open Question 2: no.

**Approach:** Use VB-Audio's signed VB-CABLE instead. Everything played to its "CABLE Input" playback device comes out of its "CABLE Output" recording device. voice-me plays each utterance to CABLE Input, and voice chat selects CABLE Output as its microphone. Settings → Dependencies gets a Windows Virtual Microphone row. When the device is missing, one Install button downloads VB-Audio's official pack (pinned, SHA-256-verified), unpacks it, and launches VB's own setup with a Windows administrator prompt. The user confirms VB's installer, which carries VB's licence. Then the row re-checks.

**Decisions (from the user, 2026-09-24):** VB-CABLE, not Virtual-Audio-Driver. One-button install with an admin prompt, running VB's own setup UI rather than a silent install (VB's licence forbids embedding the package in another installer without the author's agreement). This change covers the Windows virtual mic only. Story 3.13 (Windows' own speech engine, instead of Edge TTS on Windows) comes next; 3.16 (Piper on Windows) is deferred.

## Boundaries & Constraints

**Always:** Audio goes only to the one device whose name starts with `CABLE Input`. There is no fallback to the default output device: playing to the speakers is the failure this product exists to prevent (2.9). The AD-11 `AudioBuffer` (24 kHz mono f32) crosses `play` unchanged; resampling and channel/format conversion happen inside `voice-me-audio-windows`. Downloads follow 3.2: pinned URL, size and SHA-256; a `.part` file, verified, then renamed; progress on the row; the zip deleted after extraction. Zip entries are extracted only by their enclosed (safe) names. Failures are `VoiceMeError`s shown on the row or in the Speak notification, never a panic. The row credits VB-CABLE as VB-Audio's donationware (www.vb-cable.com), as its licence asks.

**Never:** No silent or unattended driver install. No driver bundled in voice-me's own package or release assets. No Virtual-Audio-Driver code. No startup auto-install on Windows (install needs the user's admin consent). No uninstall flow. No change to the Linux mechanism. No device-selection UI.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Speak, cable installed | One `CABLE Input` device | Utterance plays to it; an app recording from CABLE Output hears it; speakers silent | N/A |
| Speak, cable missing | No `CABLE Input` | Nothing plays; one notification naming the missing VB-CABLE | `VirtualMicUnavailable` |
| Two matching devices | Two outputs starting `CABLE Input` | Refuses to play | `VirtualMicUnavailable` naming the duplicates |
| Device rate/format | e.g. 48 kHz stereo i16/f32 | Resampled from 24 kHz, mono copied to every channel, converted to the device format | Unsupported format → `VirtualMicUnavailable` |
| Empty buffer | 0 samples | `Ok`, nothing opened | N/A |
| Row, installed | Device present | Ready: "Other applications can select CABLE Output as their microphone." | N/A |
| Row, missing | Device absent | Missing, with Install and the VB-CABLE credit | N/A |
| Install | Click Install | Download with progress → verify → extract → admin prompt → VB setup → re-check | Hash mismatch, download failure → row error; `.part` handling as in 3.2 |
| Admin prompt declined | User says No | Row: "Windows did not allow the installer to run." | Error on the row |
| Installed, not yet active | Setup finished, device absent | Row says to restart Windows, then press Check again | Missing row, detail changed |

</frozen-after-approval>

## Code Map

- `crates/voice-me-audio-windows/src/lib.rs` -- today a stub returning `VirtualMicUnavailable` (Story 2.8). Becomes the adapter: device lookup via `cpal` (WASAPI) by the `CABLE Input` name prefix (exactly one), `play` (build an output stream in the device's default config, feed resampled and converted frames, block until drained, with a timeout), `virtual_microphone_available()`, and `run_installer_elevated(setup: &Path)`. Pure helpers (name matching, mono→N channels, rubato resample, sample conversion) are testable on Linux. Mirror the 2.9 guards in `crates/voice-me-audio-linux/src/lib.rs::play` (empty buffer, exactly-one-device).
- `crates/voice-me-audio-windows/Cargo.toml` -- add `cpal = "0.18"` (as in voice-me-ui) and `rubato = "5.0.0"` (as in voice-me-tts-remote; copy its `resample` pattern from `crates/voice-me-tts-remote/src/wav.rs:59`).
- `crates/voice-me-deps/src/sources.rs` -- pin the pack: `https://download.vb-audio.com/Download_CABLE/VBCABLE_Driver_Pack45.zip`, size `1_318_877`, sha256 `b950e39f01af1d04ea623c8f6d8eb9b6ea5c477c637295fabf20631c85116bfb`, destination `<cache>/vb-cable/VBCABLE_Driver_Pack45.zip`. The `Sources` field is `Some` on Windows only (like `pinned_runtime`), so Linux fixtures can inject it.
- `crates/voice-me-deps/src/provision.rs` -- add whole-archive zip extraction into a directory (enclosed names only, `.part` then rename per file), next to `extract_runtime_library`.
- `crates/voice-me-deps/src/lib.rs` -- the `#[cfg(not(target_os = "linux"))] virtual_microphone_row` returns `None` (L747): on Windows, return the row via `voice_me_audio_windows::virtual_microphone_available()`. `provision_row`'s `VirtualMicrophone` arm (L194): when `sources.virtual_mic` is set, fetch it with progress, extract, then run the installer hook with the setup path, delete the zip, and report. `install_virtual_microphone` (L821) on Windows runs `VBCABLE_Setup_x64.exe` elevated. Widen `VirtualMicInstaller` to take the extracted directory (Linux ignores it).
- `crates/voice-me-deps/Cargo.toml` -- `voice-me-audio-windows` under `[target.'cfg(target_os = "windows")'.dependencies]`.
- `crates/voice-me-app/src/main.rs` -- L2536–2540: build the real adapter on Windows; update the stale 2.8 comments. No startup ensure on Windows.
- `crates/voice-me-deps/src/provision_tests.rs` -- fixture pattern for installs (`Fixture::with_extra_files`, fake pinned hashes, `fixture.provision(kind)`).
- Docs: `README.md` Windows virtual mic line; PRD Open Question 2 and the architecture spine's Deferred entry get the answer (VB-CABLE).

## Tasks & Acceptance

**Execution:**
- [x] `crates/voice-me-audio-windows/{Cargo.toml,src/lib.rs}` -- adapter, availability check, elevated installer launch, pure helpers with unit tests -- the Windows `VirtualMicPort`
- [x] `crates/voice-me-deps/src/{sources.rs,provision.rs}` -- VB-CABLE pin and directory extraction -- verified download
- [x] `crates/voice-me-deps/src/lib.rs`, `Cargo.toml` -- Windows row and Install flow; installer hook takes the extracted dir -- one-click install
- [x] `crates/voice-me-deps/src/provision_tests.rs` -- install via fixture (fake pack with setup + decoys, fake installer records the path, zip deleted), extraction rejects `../` entries, hash mismatch keeps no pack -- matrix rows
- [x] `crates/voice-me-app/src/main.rs` -- wire the adapter on Windows -- Speak plays through the cable
- [x] `README.md`, PRD Open Question 2, `ARCHITECTURE-SPINE.md` Deferred -- record the driver decision -- documentation stays true

**Acceptance Criteria:**
- Given Windows without VB-CABLE, when Settings → Dependencies opens, then the Virtual Microphone row is missing with Install and credits VB-CABLE
- Given that row, when Install is pressed and the admin prompt and VB's installer are accepted, then after a restart (if Windows asks) Check again shows it ready
- Given VB-CABLE installed and Discord's input set to CABLE Output, when a line is spoken through the overlay, then Discord hears it and the speakers stay silent
- Given `cargo test --workspace` on Linux and Windows CI, then everything passes and `cargo check --target x86_64-pc-windows-msvc`-level code compiles on the Windows runner

## Spec Change Log

- Windows CI after merge: the `voice-me-deps` test binary died with STATUS_ACCESS_VIOLATION. cpal 0.18.2's WASAPI host initialises COM per thread (`host/com.rs`), calls `CoUninitialize` when the thread ends, and keeps one process-wide `ENUMERATOR` created in whichever thread asked first. Once that thread exits (test threads always do, and Tokio retires idle blocking threads), the next call through the enumerator crashes; in the app that would have been a later Speak or check. Amended: every cpal call in `voice-me-audio-windows` runs on one long-lived "voice-me-audio" thread (`on_audio_thread`). KEEP: no cpal call from any other thread.

## Review Triage Log

- medium (patch): `run_installer_elevated` ignored VB setup's exit code (`Start-Process -Wait` without `-PassThru`), so a cancelled VB installer counted as success and wrote the restart marker. Found by blind, edge-case and verification layers.
- low (patch): every PowerShell failure became "Windows did not allow the installer to run" and stderr was discarded.
- medium (patch): the `setup-ran` marker was never cleared, so the row could say "restart" forever after an uninstall or a failed setup.
- low (patch): the zip was deleted before the installer ran, so a retry after a declined prompt downloaded the pack again.
- medium (patch): `play_on` dropped the stream on the first all-silence callback, while the tail could still be queued in the WASAPI endpoint, clipping the last syllable.
- medium (patch): cpal 0.18.2's WASAPI `description()` calls `.expect("could not open property store")` (`wasapi/device.rs:420`). Verified in the registry source; a bad device would panic the check or a Speak.
- low (patch): PowerShell treats ‘ ’ ‚ ‛ as single quotes, so `powershell_quote` must double them too.
- medium (patch): there was no route if VB-Audio moves the pinned pack (it cannot be mirrored). The row now names the manual download.
- gap (patch): no test tied the marker writer to its reader. Filed pre-verified by the verification layer.
- gap (patch): the real Windows installer hook was never called by a test. Filed pre-verified.
- gap (defer): the `play_on` drain/timeout logic is untested; it needs a device or a refactor. Recorded in deferred-work.md.
- medium (defer): the unpacked setup is run elevated without an Authenticode check. Hardening; recorded in deferred-work.md.
- low (defer): PRD FR-6, the addendum and the distribution claim still name the old driver. Correct-course material; recorded in deferred-work.md.
- false: "3.13/3.16 are not in epics.md". They are, at `epics.md` lines 570 and 623.
- false: "spec in-review vs sprint in-progress". The sprint status syncs at finalization, by design.
- low (rejected): `epic-2-context.md` reads as stale. It is a cache compiled from the planning docs and is recompiled automatically now that they are newer.
- low (rejected): ARM64/32-bit Windows would get the x64 setup. voice-me ships x86_64 Windows builds only.
- maybe-false (rejected, low): `[`/`]` in the setup path. Needs a Windows check of `Start-Process -FilePath` wildcard handling; the cache path under AppData rarely contains them.
- low (rejected): a user who renames a device to start with "CABLE Input". Contrived.
- low (rejected): when device enumeration fails, the row still offers Install. Rerunning VB's setup is harmless.
- low (rejected): the unpacked pack stays in the cache. VB's setup there is also its uninstaller.
- low (rejected): no tests for the symlink refusal or the skip-fetch branch, and a loose resample-length tolerance. Unlikely to matter in use.

## Design Notes

Elevation without a new Windows API crate: `powershell -NoProfile -WindowStyle Hidden -Command "Start-Process -FilePath '<setup>' -Verb RunAs -Wait"`, spawned with `CREATE_NO_WINDOW`. A declined UAC prompt makes `Start-Process` fail, and that is reported as "Windows did not allow the installer to run". Quote the path for PowerShell by doubling `'`.

Playback blocks until drained: the output callback pulls from the converted frame buffer, writes silence once it runs out and signals an `mpsc` channel, and `play` waits on that with a timeout of (duration + 2 s) before dropping the stream.

## Verification

**Commands:**
- `cargo test -p voice-me-audio-windows -p voice-me-deps` -- expected: all pass on Linux
- `cargo check --workspace --all-targets` and `cargo fmt --check` -- expected: clean
- CI `build-windows` -- expected: build and tests green

**Manual checks (if no CLI):**
- On Windows: Install from the row (admin prompt, VB setup), restart if asked, row ready; select CABLE Output in a recorder (Sound Recorder or Discord), speak a line from the overlay, and hear it in the recording with nothing from the speakers.
