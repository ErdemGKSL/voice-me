---
title: 'Speak with Piper on Windows, with eSpeak NG installed in one click'
type: 'feature'
created: '2026-09-24'
status: 'done'
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
- [x] `crates/voice-me-espeak/` -- Windows lookup, `--path`, `CREATE_NO_WINDOW`, cfgs. Tests for the portable lookup ordering helper and for `ipa_args` with and without `espeak-ng-data`.
- [x] `crates/voice-me-tts-piper/` -- cfgs and per-call program resolution.
- [x] `crates/voice-me-core/src/{state,settings_store,assets}.rs` -- Windows default, `espeak_dir`, tests.
- [x] `crates/voice-me-deps/src/{sources,capability,lib}.rs`, `Cargo.toml`, `provision_tests.rs` -- the eSpeak asset, the Windows row and the Install flow. Tests, run on Linux via injected sources and a fake unpacker:
  - the Install produces a Ready row, the `.msi` is deleted, and progress is reported;
  - on a hash mismatch nothing is installed;
  - an unpacker failure leaves no `espeak-ng` directory;
  - an unpack with no `espeak-ng.exe` is refused;
  - the Linux `SystemVoiceEngine` provision is still refused.
- [x] `crates/voice-me-app/src/main.rs` -- `build_piper` on Windows.
- [x] `README.md` -- document it.

**Acceptance Criteria:**
- Given `cargo test --workspace` on Linux, when it runs, then everything passes and no Linux test expectation changed except for new tests.
- Given CI's Windows job, when it builds and tests, then `voice-me-espeak`, `voice-me-tts-piper`, `voice-me-deps` and `voice-me-app` compile and their unit tests pass.
- Given a Windows machine with no eSpeak NG, when Piper is selected and Install is pressed on the eSpeak NG row, then the row becomes Ready and a Speak Action speaks through Piper. This is a manual check once the Windows app starts (Stories 2.8 and 2.2).

## Implementation Notes

- `voice-me-espeak` now depends on `voice-me-core`: the unpacked layout is named once in `assets` (`ESPEAK_WINDOWS_INSTALL_DIR`, `ESPEAK_WINDOWS_PROGRAM`, `espeak_program(espeak_dir)`), which both espeak (re-exported as `WINDOWS_INSTALL_DIR`/`WINDOWS_PROGRAM`/`unpacked_program`) and deps use. The Windows lookup's order lives in the portable `find_windows_program(espeak_dir, program_files, path_dirs)`.
- `msiexec` goes through the one bounded runner: a new Windows-only `voice_me_espeak::run_raw` appends `TARGETDIR="<dir>"` verbatim (`CommandExt::raw_arg`), since msiexec parses `PROPERTY="value"` itself and a cache path can hold spaces. `run`/`run_raw` share one `run_command`, which sets `CREATE_NO_WINDOW` on Windows.
- Review fixes: an `espeak-ng.msi` already in the cache is reused only if `provision::is_verified` (pinned size and SHA-256) passes, otherwise it is deleted and fetched again; the early return for an already-unpacked eSpeak NG deletes any leftover `.msi`; the msiexec arguments come from the portable `msiexec_unpack_args`, tested on every OS.
- Install reuses a verified `espeak-ng.msi` left by an earlier failed unpack (as the runtime does with its archive), removes the MSI copy an administrative image carries, and returns at once when `<cache>\espeak-ng\eSpeak NG\espeak-ng.exe` already exists.
- The Windows eSpeak row is `capability::windows_espeak_row(found, installable)`; with no pinned MSI for the target it falls back to manual steps.
- Verification here (Linux): tests pass for espeak 15, tts-piper 18+1+1, core 101, deps 97, app 53, tts-system-linux 13+1, tts-edge 22+2. `cargo fmt --check` and clippy on the changed crates are clean. `voice-me-ui` and `voice-me-tests` were only `cargo check --all-targets`ed: the disk was full (under 1 GB free), so superseded test executables for the crates under test were deleted from `target/debug/deps` to relink them (no `cargo clean`, no dependency rebuild). The Windows cross-check was not run: the target is not installed and there was no disk for it.
- Verified after the review patches (pass 1):
  - Linux tests pass: espeak 16, tts-piper 21, tts-system-linux 14, tts-edge 24, core 101, deps 101, ui 124, app 53, tests 11.
  - `cargo fmt --check` is clean.
  - `voice-me-espeak` was cross-checked for Windows with `cargo clippy --target x86_64-pc-windows-gnu --all-targets` (stub core, since gpui-kit's build script does not cross-compile here). It came out clean.
  - The deps Windows arms and the Windows-only test were read-checked only. CI `build-windows` is their first compile.
  - The disk ran out several times. Stale incremental caches and already-passed test binaries were deleted: no `cargo clean`, no dependency rebuild.

## Spec Change Log

## Review Triage Log

Pass 1 (blind-hunter, edge-case-hunter, verification-gap).

| Verdict | Finding | Evidence |
|---|---|---|
| low | A leftover `espeak-ng.msi` is reused without checking it (blind, edge, vg) | A file only gets its final name once verified, but a later pin change would reuse the old MSI. Direct fix: verify before reuse → patch |
| false | `msiexec` is resolved through PATH (blind) | Rust's Windows `Command` searches the application dir, System32 and the Windows dir before PATH, so System32's `msiexec.exe` wins → rejected |
| low | A timeout kills only the msiexec client, and the Installer service may keep writing (blind, edge) | Unpacking 25 MB takes seconds, not 120 s. Handling it needs service detection → rejected |
| low | Failures give only an exit code, with no log (blind) | Real but cosmetic. Translating codes adds branches → rejected |
| low | No "Unpacking…" state after the download (blind) | The unpack takes seconds → rejected |
| low | No tests for a network error, a stale directory, or a timeout (blind) | Network and `.part` handling are 3.2's shared `fetch`, already tested → rejected |
| maybe-false | Windows-only code never compiled (blind) | `voice-me-espeak` was cross-checked (`clippy --target x86_64-pc-windows-gnu` with a stub core, clean). The deps Windows code was read-checked. CI `build-windows` settles it after the push → tracked there |
| false | Spec and sprint status disagree (blind) | Sprint status moves at step 5 → rejected |
| low | The row text hardcodes "1.52.0 (12.8 MB)" (blind) | It would silently go stale on a pin change. Fix: a test ties it to `ESPEAK_VERSION` and the size → patch |
| low | The eSpeak layout constants exist in both deps and espeak (blind) | Install and the lookup can drift apart. Moved into `voice_me_core::assets` → patch |
| low | `Program Files (x86)` is not searched (blind) | Current eSpeak NG releases ship x64 only. It only costs a redundant download → rejected |
| false | The per-clause `find_program` changes Linux behaviour or cost (blind) | It resolves to the same program. A few stats cost microseconds, against a millisecond spawn → rejected |
| low | The fast path leaves a stray MSI behind (blind, edge) | Real, and the fix is one `remove_file` → patch |
| low | The README lacks the cache location and cleanup details (blind) | Cosmetic → rejected |
| low | Bad comment wrap in `state.rs` (blind) | Direct correction → patch |
| low | An unpack with `espeak-ng.exe` but no data counts as Ready (edge) | msiexec either unpacks all 443 files or fails → rejected |
| low | A cache copy whose data was deleted wins over Program Files (edge) | Needs someone to hand-delete files → rejected |
| medium | A non-ANSI profile path breaks `--path`, because espeak-ng.exe reads a narrow argv (edge) | Plausible for profiles outside the ANSI code page. The fix needs `GetShortPathNameW` → defer |
| false | The row ignores the check's root (edge) | The check's root is `model_cache_root()`, the same one `find_program` reads → rejected |
| medium | `EspeakPhonemizer::new()`'s per-call lookup is untested (vg) | Pre-verified → patch |
| medium | The Windows cache wiring and the row's `installable` flag are untested (vg) | Pre-verified → patch (a Windows-only test) |
| medium | `--path` is never shown reaching the program end to end (vg) | Pre-verified → patch |
| medium | The msiexec command line is untested (vg, filed defer) | A portable pure argument builder makes it testable on Linux → patch |

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
