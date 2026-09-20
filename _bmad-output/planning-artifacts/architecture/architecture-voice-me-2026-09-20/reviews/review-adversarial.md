---
name: 'review-adversarial'
type: architecture-review
paradigm: adversarial-divergence
target: ARCHITECTURE-SPINE.md (architecture-voice-me-2026-09-20)
purpose: 'Find pairs of units, one level down, that each obey every AD to the letter yet build incompatibly with each other.'
status: draft
created: '2026-09-20'
---

# Adversarial Review — ARCHITECTURE-SPINE.md

Method: for each finding, two hypothetical contributors (or crates) are each shown to satisfy every applicable AD's literal text, while producing shared-data shapes, ownership, or mutation paths that cannot both be true in one running system. Five real holes found; one additional near-miss is noted but not counted as a hole (it collapses once a small clarification is made, unlike the other five).

## Summary

| # | Hole | ADs that failed to prevent it |
| --- | --- | --- |
| 1 | No SettingsStore *adapter* crate is named — core may do fs I/O itself, or an adapter must exist to do it | AD-1, AD-6, Structural Seed |
| 2 | `AppState`'s "active Reference Voice Sample" field shape is unspecified (path vs. ID + index) | AD-3, AD-6 |
| 3 | AppEvent-channel receiver ownership is unspecified (core vs. app vs. ui) | AD-3, AD-5, AD-10 |
| 4 | Adapter-to-core reporting: direct use-case call vs. AppEvent — both readable from AD-9's own wording | AD-3, AD-9 |
| 5 | Downloaded runtime assets (deps' cache) fall outside "settings" and "user assets" — no rule says who owns that fs location | AD-6, AD-7, AD-1 |

---

## Finding 1 — `SettingsStore` has no adapter crate, so "who does the I/O" is undecided

**AD(s) that failed to prevent it:** AD-1 ("core depends on no other `voice-me-*` crate" — silent on core's own I/O), AD-6 ("behind the `SettingsStore` port that only `voice-me-core` calls"), Structural Seed (lists `HotkeyPort`/`VirtualMicPort`/`TtsPort` each with a dedicated adapter crate — `SettingsStore` gets none).

**Contributor A — core does its own I/O.**
`voice-me-core` implements `SettingsStore` concretely inside itself, using `std::fs` and the `directories` crate directly (both are external, non-`voice-me-*` dependencies, so AD-1's literal rule — "no other `voice-me-*` crate" — is not violated). Core's use-case functions (`save_reference_sample`, `load_settings`, etc.) perform real file writes synchronously. This is fully compliant with AD-6's text: settings and RVS audio live in core-owned locations, resolved via `directories`, behind a port "only `voice-me-core` calls" — and here core *is* the one calling it, because core also *is* the implementation.

**Contributor B — a dedicated adapter crate.**
A new crate, `voice-me-settings`, is created (mirroring the pattern of every other port: `HotkeyPort` → `voice-me-hotkey-*`, `VirtualMicPort` → `voice-me-audio-*`, `TtsPort` → `voice-me-tts`). It implements `SettingsStore`, depends on `voice-me-core` for the trait, and does the actual TOML/file I/O. `voice-me-core` holds only the trait definition and stays I/O-free, matching the hexagonal paradigm's opening line ("`voice-me-core` is the hexagon: platform-agnostic domain types... and the port traits" — implying core defines but does not implement ports, exactly as it does for the other three).

**Why both are "compliant" and incompatible:** Contributor A makes `voice-me-core` a crate with real side effects and a hard dependency on a temp/real filesystem for any unit test; Contributor B assumes a crate (`voice-me-settings`) that the Structural Seed never lists, and that AD-1's adapter-wiring language ("`voice-me-app` selects the right crates per target") never accounts for (this port isn't per-OS, so it doesn't fit AD-2's `cfg`-gated selection story either). Whichever contributor writes `voice-me-core`'s Cargo.toml first silently forecloses the other's design — there is nothing in the spine that says which one is correct, and nothing that would fail CI if both partially existed (e.g. one dev half-implements the trait in core "for now" while another opens a `voice-me-settings` skeleton crate).

**Suggested tightened Rule (extend AD-1 or AD-6):**
> `SettingsStore` is implemented by a dedicated adapter crate, `voice-me-settings`, exactly like every other port in this spine — never inline inside `voice-me-core`. `voice-me-core` performs no direct filesystem, network, or process I/O of any kind; if a future port lacks a per-OS split, it still gets its own single adapter crate. Add `voice-me-settings` to the Structural Seed and to the dependency graph in the Design Paradigm diagram.

---

## Finding 2 — "Active Reference Voice Sample" has no defined shape in `AppState`

**AD(s) that failed to prevent it:** AD-3 (names "active Reference Voice Sample" as an `AppState` field but not its type), AD-6 (says RVS audio files live at the OS data directory, but not whether more than one may exist or how "active" is tracked).

**Contributor A — single-slot path.**
`AppState.active_reference_sample: PathBuf`, pointing directly at one file in the data directory. `SettingsStore::save_reference_sample(bytes) -> PathBuf` overwrites the one file (or writes a new one and updates the TOML's stored path) each time FR-1's "re-record or replace... at any time" fires. This satisfies AD-6 literally: one TOML settings file, RVS audio at the OS data directory, mutated only through core.

**Contributor B — indexed samples.**
Because FR-1's consequence ("re-record or replace... at any time") is read as implying a history/multiple candidates (e.g. so a bad re-record doesn't silently destroy the previous good clip until the user confirms), `AppState.active_reference_sample: SampleId(Uuid)` with a `samples/index.toml`-style manifest in the data directory mapping IDs to files, metadata (created_at, duration), and a separate `active_sample_id` field in the main settings TOML. This is equally compliant with AD-6's literal text — it's still "one TOML settings file... and Reference Voice Sample audio files at the OS data directory... behind the `SettingsStore` port."

**Why both are compliant and incompatible:** the `SettingsStore` port's method signatures differ irreconcilably (`PathBuf` in/out vs. `SampleId` in/out plus a second manifest file format), so a `voice-me-ui` built against Contributor A's port cannot compile or reason correctly against Contributor B's, and vice versa — yet neither design breaks any AD as written. (The spine's own Deferred section defers "exact settings TOML schema," which covers field *names/types* inside one file, but not this structural fork between single-path and ID+manifest.)

**Suggested tightened Rule (extend AD-6):**
> `AppState.active_reference_sample` is a `PathBuf` into the OS data directory; v1 keeps exactly one stored Reference Voice Sample file (re-record/replace overwrites it in place — no history, no manifest, no multi-sample index). Any future multi-sample library is a v2 architectural change to this AD, not an implementation detail left to whoever writes `SettingsStore` first.

---

## Finding 3 — Who owns the receiving end of the single `AppEvent` channel?

**AD(s) that failed to prevent it:** AD-3 ("every adapter signals the app through one shared `AppEvent` enum over one channel; `voice-me-ui`... is the only consumer that turns `AppEvent`s into GPUI entity updates, via `cx.spawn`"), AD-5 (bridging is "deliberate" but doesn't say where), AD-10 ("its result reaches the UI later via `AppEvent`").

**Contributor A — `voice-me-app` (composition root) owns the receiver.**
`voice-me-app::main` creates the `mpsc` channel, hands clones of the sender to every adapter it wires up (per its Structural Seed role: "composition root... per-OS adapter wiring"), and itself runs the receive loop, calling `voice-me-core` use-case functions to mutate `AppState` as events arrive, then separately notifying `voice-me-ui` (e.g. via a GPUI global or a second internal signal) that state changed. This satisfies AD-3's text: adapters signal "the app" (read as `voice-me-app`), and `voice-me-ui` is still the only thing that turns *updates* into GPUI entity mutations — it just does so by observing `AppState`, not by draining `AppEvent` itself.

**Contributor B — `voice-me-ui` owns the receiver directly.**
`voice-me-ui` is built as "the only consumer" of `AppEvent` in the literal sense: it holds the `Receiver<AppEvent>`, drains it inside a `cx.spawn` task (matching AD-3's explicit "via `cx.spawn`"), and calls `voice-me-core` use-case functions itself to mutate `AppState` before updating its own GPUI entities in the same task. `voice-me-app` only constructs the channel and passes the sender half to adapters and the receiver half to `voice-me-ui` at startup — it never touches `AppEvent` itself.

**Why both are compliant and incompatible:** in A, the mutation of `AppState` in response to adapter events happens on whatever thread/task `voice-me-app`'s loop runs on, decoupled from GPUI's executor, with `voice-me-ui` only ever reading already-updated state; in B, the mutation happens *inside* a GPUI `cx.spawn` task owned by `voice-me-ui`, meaning `voice-me-ui` (a UI/adapter crate per the Design Paradigm's own "driving adapters... the UI") ends up calling core's mutating use-cases directly — which is exactly the kind of thing AD-3 is trying to centralize, yet nothing forbids it since AD-3 literally names `voice-me-ui` as the one that acts on `AppEvent`. Two contributors building adapters against "the" `AppEvent` sender will disagree about latency assumptions, panics-on-disconnected-receiver behavior, and where a mutation "commits" relative to a GPUI frame — with no AD to arbitrate.

**Suggested tightened Rule (extend AD-3):**
> The `AppEvent` channel's receiving end is owned by `voice-me-core`'s own event loop (spawned by `voice-me-app` at startup), not by `voice-me-ui`. Core drains events, applies the corresponding use-case mutation to `AppState`, and then republishes state changes on its own notification mechanism (e.g. a `watch` channel or GPUI global) that `voice-me-ui` subscribes to from within `cx.spawn`. `voice-me-ui` never holds the `AppEvent` receiver and never calls a mutating use-case function directly from adapter-signaled input — only from direct user action within the UI itself (e.g. hitting Enter).

---

## Finding 4 — AD-9's "reports the result to core" is ambiguous between a direct call and an event

**AD(s) that failed to prevent it:** AD-3 (adapters "signal the app through one shared `AppEvent`... no adapter mutates it directly — only through `voice-me-core` use-case functions" — these two clauses describe two different call shapes that AD-9 doesn't disambiguate between), AD-9 itself ("`voice-me-deps` detects GPU acceleration... and reports the result to `voice-me-core`, which records it in `AppState`").

**Contributor A — direct synchronous use-case call.**
`voice-me-deps`, after its Dependency Check runs (potentially inside its own Tokio task per AD-5), calls `voice_me_core::record_gpu_capability(caps)` directly — a plain function call across the crate boundary, which is allowed structurally since `voice-me-deps` already depends on `voice-me-core` per AD-1. This reads AD-9's "reports the result to `voice-me-core`" as literally meaning "calls into core."

**Contributor B — goes through `AppEvent` like every other adapter.**
`voice-me-deps` instead sends `AppEvent::GpuCapabilityDetected(caps)` on the shared channel, deferring the actual `AppState` mutation to whatever consumes `AppEvent` (per AD-3's general rule that *every* adapter signals through the one channel — AD-9 names no exception to that).

**Why both are compliant and incompatible:** if even one adapter in the system (here, `voice-me-deps`) is built by a contributor reading AD-9 as license for a direct call while every other adapter goes through `AppEvent`, `voice-me-core`'s `AppState` now has two distinct entry points for mutation — one synchronous/direct-call, one async/event-driven — active at once. That's precisely the "divergent... callback or polling mechanism" AD-3 says it exists to prevent, yet AD-9's own wording is the thing that invites the exception.

**Suggested tightened Rule (amend AD-9):**
> `voice-me-deps` reports its Dependency Check result — including GPU capability — as an `AppEvent` variant on the shared channel (AD-3), never as a direct call into a `voice-me-core` mutating use-case function. "Reports to core" in this AD means "emits an `AppEvent` that core's event loop turns into an `AppState` mutation," with no exception for this or any other adapter.

---

## Finding 5 — Downloaded runtime assets have no owning rule at all

**AD(s) that failed to prevent it:** AD-6 (scopes "core-owned locations" to "settings" and "Reference Voice Sample audio" only), AD-7 (governs the remote *source* of these assets — GitHub Releases — but not their local resting place), AD-1 (the general "everything through core" spirit, not stated as a filesystem rule).

**Contributor A — `voice-me-deps` owns its own cache silently.**
Reading AD-6 literally — it names two things as core-owned (settings TOML, RVS audio) and says adapters "never write to the data directory themselves" only in that specific context — `voice-me-deps` concludes that the bundled Python runtime, GPU libraries, and Windows driver installer it downloads from GitHub Releases (AD-7) are neither settings nor user assets, so it resolves its own cache location via `directories::ProjectDirs::cache_dir()` and writes there directly, with no call into `voice-me-core` at all. `voice-me-core`'s `AppState` has no notion of where these files are.

**Contributor B — extends `SettingsStore`/core-ownership to all local persistent state.**
Reading AD-6's *prevents* clause — "an adapter reading or writing... directly and drifting from what `voice-me-core` believes is true" — as the operative intent, this contributor treats it as covering any locally-persisted artifact the app depends on, not just the two named examples. `voice-me-deps` hands downloaded bytes to `voice-me-core` (via a broadened `SettingsStore`-like port) to write and to record the resulting path in `AppState`, so core always "believes what's true" about what's on disk.

**Why both are compliant and incompatible:** Contributor A produces an untracked, core-invisible filesystem side channel (undermining the very drift AD-6 exists to prevent, but without technically breaking its narrower literal scope); Contributor B forces multi-hundred-MB model weights and installer binaries through a port designed for a TOML file and a short audio clip, and requires `AppState`/the settings schema to somehow represent large binary caches — a very different shape than Finding 2's RVS field. Whichever crate is implemented first sets a precedent the other can't interoperate with (e.g. `voice-me-core`'s Dependency Check status in `AppState`, used by AD-9, needs to know installed-or-not for GPU libs — which requires *some* answer to "where do these live and who tracks it").

**Suggested tightened Rule (new clause on AD-6, or a new AD-6a):**
> "Core-owned locations" cover *all* locally-persisted application state, not only settings and the Reference Voice Sample: this includes `voice-me-deps`'s downloaded runtime assets (Python runtime, model weights, driver installers). `voice-me-deps` resolves its cache directory via a `DependencyCachePort` (or an extended `SettingsStore`) exposed by `voice-me-core`, and records installed-version/location metadata in `AppState` after every provisioning action — it does not write to a `directories`-resolved path of its own choosing. Large binary payloads are written by `voice-me-deps` itself (no need to funnel bytes through core), but the *path and installed-state bookkeeping* always goes through core, exactly as AD-6 already requires for settings and RVS audio.

---

## Near-miss (not counted as a hole)

**AD-4 domain error enum vs. AD-3 `AppEvent` enum — are TTS/deps failures carried as an `AppEvent` variant wrapping the domain error, or surfaced some other way?** On inspection this collapses under AD-10's own text ("failures surface to `voice-me-core` only as the domain error enum") read together with AD-3's "every adapter signals... through `AppEvent`" — the only self-consistent reading is that `AppEvent` has a variant carrying the AD-4 error type (e.g. `AppEvent::TtsFailed(Error)`), and no second contributor has a textually-supported alternative construction. Flagged here so the reviewer doesn't have to re-derive it, but it is not offered as one of the five holes above.
