---
title: 'Speak with Piper on Windows, with eSpeak NG installed in one click'
type: 'feature'
created: '2026-09-24'
status: 'in-progress'
baseline_commit: 'aedc7a52c82845ea0ed7853c63bf79d5389ff1ce'
route: 'dispatch'
review_loop_iteration: 0
context: ['{project-root}/_bmad-output/implementation-artifacts/epic-3-context.md', '{project-root}/_bmad-output/implementation-artifacts/spec-3-15-speak-naturally-and-instantly-with-piper-on-linux.md']
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** Piper (Story 3.15) runs only on Linux. On Windows, a Piper selection is refused with "arrives in a later voice-me release", and Windows' first-run default is still the 1.56 GB bundled CPU backend.

**Approach:** Build the same `voice-me-tts-piper` engine on Windows. `voice-me-espeak` runs `espeak-ng.exe` as a separate process, which it finds in voice-me's cache, under `C:\Program Files\eSpeak NG\`, or on PATH. For a Piper selection, Settings → Dependencies gets an eSpeak NG row. When eSpeak NG is missing, one Install fetches the official eSpeak NG 1.52.0 `espeak-ng.msi` (pinned URL, size and SHA-256) and unpacks it. Piper becomes Windows' first-run default (AD-9).

**Decision (from the user, 2026-09-24):** Install unpacks the MSI into voice-me's cache with `msiexec /a espeak-ng.msi /qn TARGETDIR=<cache>\espeak-ng.tmp`, with no admin prompt and no registry or PATH changes. The spec is kept whole even though it runs over the token guideline.

## Boundaries & Constraints

**Always:** Phonemes for a clause come from the eSpeak NG program over stdin/stdout, as on Linux. Nothing of eSpeak is linked. The pinned download is `https://github.com/espeak-ng/espeak-ng/releases/download/1.52.0/espeak-ng.msi`, 12 765 862 bytes, sha256 `7f673c709ea5dd579d3b5ebb98688cc575328a6ab7438d2bc405b88cedaeafb9`. It follows 3.2's download rules: a `.part` file, verified, then renamed, with progress on the row, and the `.msi` deleted afterwards. A program found next to an `espeak-ng-data` directory is always run with `--path=<its directory>`. Every child process on Windows is spawned with `CREATE_NO_WINDOW`. Failures are `VoiceMeError`s on the row or in the Speak notification, never a panic. Linux behaviour and Linux tests stay unchanged.

**Never:** No bundling of eSpeak NG in voice-me's own release assets. No eSpeak row for the Windows System Voice selection (Story 3.13 uses Windows' own engine). No uninstall flow. No changes to the Story 2.8 virtual-microphone code (another session owns it). No Piper GPU path.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| First run, Windows | No saved backend | Piper selected, fahrettin voice | N/A |
| eSpeak missing | Piper selected, no `espeak-ng.exe` found | Speech-blocking eSpeak NG row, Missing, with Install | N/A |
| Install | Click Install | Download with progress → verify → unpack → row Ready ("Found at …") | Hash or size mismatch, network error, unpack failure → row error naming the cause; no half-installed program is left behind |
| Existing install | `C:\Program Files\eSpeak NG\espeak-ng.exe` or on PATH | Row Ready; nothing downloaded | N/A |
| Speak | Piper, voice and eSpeak present | Same audio path as Linux (22.05 kHz → 24 kHz) | eSpeak failure → one notification naming Piper and the reason |
| Linux | Any | Unchanged | Unchanged |

</frozen-after-approval>

## Code Map

- `crates/voice-me-espeak/src/lib.rs` -- today it is `#![cfg(target_os="linux")]` (`:25`), because the tests use `std::os::unix`.
  - Make the crate `any(linux, windows)` and gate the fake-`/bin/sh` tests with `#[cfg(unix)]`.
  - `find_program` (`:48-59`) stays as it is on Linux. On Windows it looks, in order, for `<cache>\espeak-ng\eSpeak NG\espeak-ng.exe` (the cache root comes from `voice_me_core::assets`), then `%ProgramFiles%\eSpeak NG\espeak-ng.exe`, then `espeak-ng.exe` on PATH, using `is_file()`. Put the ordering in a pure helper that takes those directories, so it is testable on Linux.
  - `ipa_args` (`:65`) gains the program path: when the program's directory holds `espeak-ng-data`, add `--path=<dir>`. On Linux `/usr/bin` never holds that directory, so its args are unchanged.
- `crates/voice-me-espeak/src/process.rs` -- `run` (`:53-121`) is portable std. Add `creation_flags(CREATE_NO_WINDOW)` under `cfg(windows)`.
- `crates/voice-me-espeak/src/os_release.rs` -- stays Linux-only; gate it with `cfg(linux)` if it no longer compiles cleanly. `voice-me-tts-system-linux` and `voice-me-tts-edge` re-export and use it and must keep compiling as they are.
- `crates/voice-me-tts-piper/{Cargo.toml:30-32, src/lib.rs:56-88}` -- widen the `voice-me-espeak` dependency and the `EspeakPhonemizer` cfgs to `any(linux, windows)`. The phonemizer resolves its program on each call (`find_program()`, falling back to `PROGRAM`), so an eSpeak installed after the engine was built is picked up. `tests/real_engine.rs` stays Linux-only.
- `crates/voice-me-core/src/state.rs:659-667` -- the `Default` becomes Piper on `any(linux, windows)`. Update the test at `:1764`, `settings_store.rs:1083-1102`, and the comments at `:867` and `:1202`.
- `crates/voice-me-core/src/assets.rs` -- add `espeak_dir(root)` = `<root>/espeak-ng` next to `piper_dir`.
- `crates/voice-me-deps/src/capability.rs`:
  - The Piper arm (`:370-381`) returns `None` on Windows too.
  - Add a Windows eSpeak row. It reuses `system_voice_engine_row`'s Ready form; the Missing form is automatable (Install) and says: "eSpeak NG is not installed. Piper reads text through it. Install downloads eSpeak NG 1.52.0 (12.8 MB) from its official release."
- `crates/voice-me-deps/src/lib.rs`:
  - `piper_rows` (`:353-369`) allows Windows, with the eSpeak row coming from the Windows lookup.
  - Add a `cfg(windows)` `system_voice_rows` sibling used for Piper only. The SystemVoice selection on Windows still gets no eSpeak row.
  - `provision_row`'s `SystemVoiceEngine` arm (`:206-208`): when `sources.espeak` is set, fetch it with progress, then call an injectable `EspeakUnpacker` hook (`fn(&Path msi, &Path target) -> Result<(), VoiceMeError>`, modelled on `VirtualMicInstaller`). Unpack into `<cache>\espeak-ng.tmp`, check that `eSpeak NG\espeak-ng.exe` exists, swap it into `espeak-ng`, then delete the `.msi`. Linux keeps the current error.
  - The Windows hook runs `msiexec /a` (the Decision above) with a 2-minute deadline and `CREATE_NO_WINDOW`. A non-zero exit reports its code.
  - Add the new code next to the existing arms; don't restructure. The 2.8 session is editing the `VirtualMicrophone` arm and `virtual_microphone_row` in the same file.
- `crates/voice-me-deps/src/sources.rs:193-313` -- add `espeak: Option<Asset>` to `Sources`, `Some` on Windows x64 only (as `pinned_runtime` does per OS), so that Linux tests can inject it. Add the field at the end of the struct. 2.8 adds `virtual_mic` there too.
- `crates/voice-me-deps/Cargo.toml:47-59` -- `voice-me-espeak` under the Windows target as well.
- `crates/voice-me-deps/src/provision_tests.rs` -- the fixture pattern (`Fixture::with_extra_files`, fake pinned hashes, `fixture.provision(kind)`).
- `crates/voice-me-app/src/main.rs:604-627` -- widen `build_piper`'s cfg to `any(linux, windows)`, and keep the stub for other OSes. Leave the virtual-mic lines (`952-1046`, `2509-2564`) alone.
- `README.md` -- the Windows Piper and eSpeak NG line.

## Tasks & Acceptance

**Execution:**
- [ ] `crates/voice-me-espeak/` -- Windows lookup, `--path`, `CREATE_NO_WINDOW`, cfgs. Tests for the portable lookup ordering helper and for `ipa_args` with and without `espeak-ng-data`.
- [ ] `crates/voice-me-tts-piper/` -- cfgs and per-call program resolution.
- [ ] `crates/voice-me-core/src/{state,settings_store,assets}.rs` -- Windows default, `espeak_dir`, tests.
- [ ] `crates/voice-me-deps/src/{sources,capability,lib}.rs`, `Cargo.toml`, `provision_tests.rs` -- the eSpeak asset, the Windows row and the Install flow. Tests, run on Linux via injected sources and a fake unpacker:
  - the Install produces a Ready row, the `.msi` is deleted, and progress is reported;
  - on a hash mismatch nothing is installed;
  - an unpacker failure leaves no `espeak-ng` directory;
  - an unpack with no `espeak-ng.exe` is refused;
  - the Linux `SystemVoiceEngine` provision is still refused.
- [ ] `crates/voice-me-app/src/main.rs` -- `build_piper` on Windows.
- [ ] `README.md` -- document it.

**Acceptance Criteria:**
- Given `cargo test --workspace` on Linux, when it runs, then everything passes and no Linux test expectation changed except for new tests.
- Given CI's Windows job, when it builds and tests, then `voice-me-espeak`, `voice-me-tts-piper`, `voice-me-deps` and `voice-me-app` compile and their unit tests pass.
- Given a Windows machine with no eSpeak NG, when Piper is selected and Install is pressed on the eSpeak NG row, then the row becomes Ready and a Speak Action speaks through Piper. This is a manual check once the Windows app starts (Stories 2.8 and 2.2).

## Implementation Notes

## Spec Change Log

## Review Triage Log

## Design Notes

**Why `--path` and not the registry:** `espeak-ng` on Windows finds its data through `--path`, then `ESPEAK_DATA_PATH`, then `HKLM\Software\eSpeak NG\Path`, the last of which only a full install writes. Passing `--path=<exe dir>` whenever `espeak-ng-data` sits beside the program makes an unpacked copy and a Program Files install behave the same way.

**MSI layout (verified with msitools):** the MSI puts `INSTALLDIR` = `ProgramFiles64Folder\eSpeak NG`, and `ProgramFiles64Folder` maps to `.`. An administrative image therefore lands at `TARGETDIR\eSpeak NG\{espeak-ng.exe, libespeak-ng.dll, espeak-ng-data\…}`: 443 files, about 25 MB. The package's `ProductVersion` reads 1.51.0, although the release tag is 1.52.0.

## Verification

**Commands:**
- `cargo test -p voice-me-espeak -p voice-me-tts-piper -p voice-me-core -p voice-me-deps -p voice-me-app` -- expected: all pass on Linux.
- `cargo fmt --check` -- expected: clean.
- `cargo check --target x86_64-pc-windows-gnu -p voice-me-espeak -p voice-me-tts-piper`, if the target can be added -- expected: compiles.
- CI `build-windows` on the change -- expected: green.

**Manual checks (if no CLI):**
- On Windows: select Piper, press Install on the eSpeak NG row, then speak a Turkish line and hear it through the virtual mic.
