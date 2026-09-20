---
name: 'review-version-check'
type: architecture-review
reviews: '_bmad-output/planning-artifacts/architecture/architecture-voice-me-2026-09-20/ARCHITECTURE-SPINE.md'
scope: 'Stack section + every version/technology claim'
created: '2026-09-20'
---

# Version/Technology Verification Review — voice-me Architecture Spine

## Method

Ran targeted web searches and direct API/GitHub-source fetches (crates.io API, docs.rs, the Rust blog, and the actual source of `zed-industries/zed` and `longbridge/gpui-kit` via `gh api`) against every version/technology claim in the Stack section and its supporting Deferred notes. Did not re-derive from training data alone; every claim below is either confirmed against a live source or flagged because it wasn't (and wouldn't have been catchable from general knowledge).

## Overall Verdict: Some issues

Every explicit version number in the Stack table checked out as accurate and current as of 2026-09-20. However, spot-checking the load-bearing GPUI/gpui-kit/gpui_tokio claims (as instructed, since these are the unusual, pre-1.0, highest-risk items) surfaced a real mechanism-level problem the spine does not surface: the specific way `gpui-kit` sources GPUI is incompatible, as stated, with the specific way `gpui_tokio` sources GPUI — a genuine risk to AD-5 that wasn't caught by the document's own "confirmed as Zed's own pattern" framing.

## Findings

### 1. [High] AD-5's "depend on Zed's own `gpui_tokio` crate (git dependency)" option is likely not viable as described, and this isn't flagged as a risk

- Fetched the real source of `crates/gpui_tokio/Cargo.toml` and `gpui_tokio.rs` in `zed-industries/zed`. Confirmed: the crate is real, and it does exactly what AD-5 claims — `Tokio::spawn(cx, future)` runs a future on a Tokio runtime and returns a GPUI `Task` via `cx.background_spawn`, i.e., it is a genuine, working Tokio↔GPUI bridge.
- But its `Cargo.toml` declares `gpui.workspace = true` — it is a **workspace-internal path dependency** inside the zed monorepo, not an independently versioned/publishable crate. There is no `gpui_tokio` entry on crates.io (confirmed: 404 from the crates.io API).
- Meanwhile, `gpui-kit` — the crate this architecture commits the app to depending on *exclusively* ("the app depends on gpui-kit only, never GPUI directly") — does **not** get GPUI from `zed-industries/zed` via git at all. Its own `Cargo.toml` (fetched directly) pins `gpui = { package = "gpui-pre", version = "0.3.5" }`, where `gpui-pre` is a third-party crates.io package described on its own listing as "gpui-pre snapshot of zed@d89e9c2" — a frozen snapshot of one specific zed commit, republished to crates.io by gpui-kit's own maintainer (not the Zed team), independent of both the official `gpui` 0.2.2 crates.io release and zed's live git `main`.
- Consequence: if `voice-me-app` were to add `gpui_tokio` as a git dependency on `zed-industries/zed` (per AD-5's first option), Cargo would resolve **two distinct, incompatible `gpui` crate instances** into the dependency graph — gpui-kit's frozen `gpui-pre@0.3.5` snapshot, and zed's live-HEAD `gpui` pulled in transitively by `gpui_tokio`. Rust treats these as different types even if versions matched, so a `cx: &App` obtained from a gpui-kit view could not be passed into `gpui_tokio::Tokio::spawn`, which expects zed's own `App`/`AppContext`. This would surface as a hard type-mismatch compile error, not a subtle bug — but only once someone tries to wire it up.
- Checked whether gpui-kit already ships its own bridge that would sidestep this: grepped `longbridge/gpui-kit`'s `crates/kit` and `crates/base` `Cargo.toml`s and did a GitHub code search for `gpui_tokio` across that repo — zero references. No ready-made compatible bridge exists today.
- **Recommendation:** AD-5's Deferred item ("whether voice-me depends directly on Zed's `gpui_tokio` crate ... or replicates its bridging approach in-house") should not be left as a coin-flip between two equally-viable options. Based on this research, "depend directly on `gpui_tokio`" is very likely a dead end given gpui-kit's actual dependency shape, and the realistic path is replicating `gpui_tokio`'s ~80-line pattern (a `Global` holding a `tokio::runtime::Handle`, plus a `Tokio::spawn` that wraps `cx.background_spawn`) against gpui-kit's own re-exported `gpui` types. That's a small, low-risk reimplementation, but the spine should say so rather than presenting both options as open.

### 2. [Medium] Stack table's GPUI row describes a sourcing mechanism the app will never actually exercise, and omits the mechanism it will

- The Stack table says: "GPUI — git dependency on `zed-industries/zed`, pinned to a specific rev/tag." That's accurate only for a hypothetical direct dependency, which AD-2/the architecture text elsewhere explicitly rules out ("the app depends on gpui-kit only, never GPUI directly").
- The GPUI version actually in the app's dependency graph is whatever `gpui-kit` 0.6.4 pins — currently the `gpui-pre@0.3.5` crates.io snapshot described above, not a git dependency at all.
- This matters for two reasons beyond pedantry: (a) it changes the provenance/trust story AD-7/AD-8 care about — this is a third-party-republished snapshot of Zed's code, not Zed's own artifact or the official crates.io `gpui` release; (b) the Deferred item "Exact GPUI git rev/tag to pin — resolved when the workspace Cargo.toml is first written" is likely moot, since there may be no GPUI git dependency for `voice-me` to pin at all — `gpui-kit`'s own Cargo.toml already fixes that version, and bumping GPUI means bumping `gpui-kit`, not editing a git rev in `voice-me`'s own `Cargo.toml`.
- **Recommendation:** correct the Stack row to describe how GPUI actually arrives (via gpui-kit → `gpui-pre` snapshot on crates.io), and drop or rescope the "exact GPUI git rev/tag to pin" Deferred item.

### 3. [Confirmed accurate] Rust 1.98.1, edition 2024

Verified against the official Rust blog: 1.98.0 shipped 2026-08-20, followed by the 1.98.1 patch release on 2026-09-03 (a vtable-miscompilation fix). Edition 2024 has been stable since Rust 1.85 (Feb 2025), so pairing it with 1.98.1 is valid and current as of today (2026-09-20). No issue.

### 4. [Confirmed accurate] gpui-kit 0.6.4 on crates.io

Queried the crates.io API directly: published versions are 0.1.0, 0.6.0, 0.6.1, 0.6.2, 0.6.4, with 0.6.4 as both `max_version` and `newest_version` — i.e., it is genuinely the latest release today, not stale. Cross-checked the crate's own `Cargo.toml` in `longbridge/gpui-kit`: workspace version is `0.6.4` throughout, and the package description ("GPUI Kit: one dependency for building desktop applications with GPUI, GPUI Base, GPUI Component and default assets") matches the `gpui_kit::component` / `gpui_kit::base` / `gpui_kit::assets` shape this repo's own `gpui-kit` skill describes. No issue with the version number itself (see Finding 2 for the sourcing-mechanism caveat).

### 5. [Confirmed accurate, and well-founded rather than just hedging] "crates.io 0.2.2 is stale — do not use"

Verified crates.io `gpui` 0.2.2 is real and is in fact the newest version crates.io has (published 2025-10-22) — so "stale" isn't about there being a newer crates.io release available; it's about drift from zed's git `main`. Confirmed via `zed-industries/zed` issue #46486, a live bug report specifically about the crates.io 0.2.2 build failing on updated macOS (linked against an older Metal SDK) while a build from git `main` works fine — i.e., independent evidence that the officially-published crate genuinely lags the git source in ways that bite users today. The spine's caution here is correct and specifically substantiated, not boilerplate.

### 6. [Confirmed accurate] thiserror 2.0.20

Confirmed via docs.rs's "latest" redirect and crates.io: 2.0.20 is the current latest published version.

### 7. [Confirmed accurate] anyhow 1.0.103

Confirmed via docs.rs: 1.0.103 is a real, currently-latest published version (released 2026-06-25).

### 8. [Low] Inconsistent pinning policy between exact-patch and "pin at implementation time" rows

`thiserror` and `anyhow` are pinned to exact current patch versions (2.0.20 / 1.0.103), while `tokio`, `tracing`, and `directories` are deliberately left as "latest stable — pin at implementation time" specifically to avoid baking in a version that goes stale before implementation starts. Since this spine carries no committed implementation date, the two hard-pinned crates are exposed to exactly the same staleness risk the other three are hedged against — a `cargo add thiserror` at implementation time could easily land past 2.0.20. Not a factual error (both numbers are real, current versions as of today), just an inconsistency worth normalizing: either mark all five "latest stable, re-check at implementation time" and drop the specific patch numbers, or explicitly timestamp the two pinned ones as "current as of 2026-09-20, re-verify before use."

### 9. [Confirmed accurate] `directories` crate as "the standard way to resolve OS config/data dirs in Rust"

Verified via web search: `directories` is the documented, actively-recommended higher-level crate for exactly this use case (per-application config/cache/data path resolution across Linux/XDG, Windows Known Folder API, and macOS Standard Directories), distinct from and recommended over the lower-level `dirs`/`dirs-next` crates for multi-path, per-app needs like AD-6's (a settings TOML in the config dir, Reference Voice Sample audio in the data dir). No dispute with the claim.

## Summary Table

| Claim | Verdict | Confidence |
| --- | --- | --- |
| Rust 1.98.1, edition 2024 | Accurate | High (official Rust blog) |
| GPUI: git dep on zed-industries/zed, crates.io 0.2.2 stale | Version-staleness claim accurate; sourcing-mechanism description is wrong for how gpui-kit actually pulls GPUI | Mixed — see Findings 1–2 |
| gpui-kit 0.6.4 on crates.io | Accurate | High (crates.io API + source) |
| thiserror 2.0.20 | Accurate | High |
| anyhow 1.0.103 | Accurate | High |
| Zed bridges Tokio via `gpui_tokio` | Crate and mechanism are real; but depending on it directly from outside the zed workspace is likely blocked by a gpui type-identity conflict with gpui-kit's `gpui-pre` snapshot — not surfaced in the spine | Real gap — Finding 1 (High) |
| `directories` is the standard OS config/data dir crate | Accurate | High |
