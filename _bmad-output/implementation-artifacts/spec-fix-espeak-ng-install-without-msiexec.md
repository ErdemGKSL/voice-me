---
title: 'Install eSpeak NG on Windows without msiexec'
type: 'bugfix'
created: '2026-09-24'
status: 'done'
route: 'oneshot'
review_loop_iteration: 0
context: []
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** On Windows, Install on the eSpeak NG row fails with "Could not unpack eSpeak NG: msiexec exit code: 1603". The package's summary information marks it as needing elevated rights (WiX, per-machine), so `msiexec /a ... /qn` cannot raise the administrator prompt and fails. Users who want eSpeak NG cannot install it.

**Approach:** Stop using `msiexec`. voice-me reads the verified MSI itself: its `File`, `Component` and `Directory` tables give each file's path, and its embedded cabinet holds the bytes. It unpacks the files into the same staging layout (`eSpeak NG\espeak-ng.exe`, `espeak-ng-data\…`), so the rest of the Install flow is unchanged. The extraction is plain Rust, runs on every OS, and needs no administrator prompt.

</frozen-after-approval>

## Implementation Notes

- Root cause:
  - The MSI's summary information has `Word Count` = 2: compressed, with elevated rights required (bit 3 clear).
  - `msiexec /a /qn` therefore cannot raise UAC, and exits with 1603.
- New `crates/voice-me-deps/src/msi_unpack.rs`:
  - The `msi` crate reads the `File`, `Component` and `Directory` tables and the embedded `#cab1.cab` stream.
  - `DefaultDir` target long names are used. `.` directories map onto their parent, and the root is `target`.
  - Every name is checked so it cannot climb out of `target`.
  - It is the default `espeak_unpacker`. The staging, verify and swap flow in `provision_espeak` is unchanged.
- Cabinet crate: upstream `cab` 0.6's `read_file` restarts the MSZIP folder for every file. For 443 files in one folder that took 134 s in a debug test.
  - At the user's suggestion, the dependency is now the `cab-superewald` 0.6.1 fork, as `cab`. Its `all_files()` reads a folder in one pass, and the whole package now unpacks in under 1 s.
- Removed:
  - `msiexec` (`unpack_espeak_msi`, `msiexec_unpack_args` and its test, `ESPEAK_UNPACK_DEADLINE`) and `voice_me_espeak::run_raw`.
  - The deletion of the admin image's MSI copy.
  - Stale "administrative image" comments. README updated.
- Verified:
  - The real pinned MSI (`VOICE_ME_ESPEAK_MSI=<path> cargo test -p voice-me-deps --lib real_espeak`) unpacks 443 files, byte-identical to `msiextract`'s output.
  - Tests pass: deps 117, espeak 16, core 101, tts-piper 21, app 53. `fmt` and `clippy` are clean.
  - Not run on Windows here. The code has no Windows-only parts now.

## Review Triage Log

Blind hunter, pass 1.

| Verdict | Finding | Evidence |
|---|---|---|
| medium | The end-to-end unpack test only runs with `VOICE_ME_ESPEAK_MSI` set, so it does not run in CI | Real. Building a synthetic MSI and cab in a test is not a small fix → defer |
| false | `row["col"]` panics on a missing column | Only the SHA-256-pinned package reaches `unpack`, and its tables have these standard columns → rejected |
| low | `all_files` swallows folder errors, giving a misleading "in none of the cabinets" | Only for a damaged package, which the SHA-256 check rules out → rejected |
| false | Windows device names and illegal characters are not refused | The pinned package has none → rejected |
| low | Stale "administrative image" and "copy of the package" comments | Direct correction → patched |
| false | The unpack has no deadline | It now takes under 1 s → rejected |
| low | Component conditions, `File.Attributes` and duplicate destinations are ignored | The pinned package has one unconditional x64 file set → rejected |
| low | The cabinet is copied into memory | 12 MB, once → rejected |
| low | The error message lacks a final period | Direct correction → patched |
| false | The spec is incomplete | Filled at finalize → rejected |
| low | The fork is lightly maintained | The Cargo.toml comment records why it is used over upstream → rejected |
| low | README wording | Cosmetic → rejected |
