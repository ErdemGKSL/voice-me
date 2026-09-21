# voice-me

Project scaffold in progress — being defined step by step with BMAD-METHOD.

## Skills

- BMAD-METHOD agents/workflows (`bmad-*` skills) for planning, PRD, architecture, sprint work.
- `gpui-kit` / `gpui-kit-design-guides` for GPUI (Rust) UI components.
- Skills live under `.agents/skills/` and are symlinked into `.claude/skills/`.

## Builds

- **Never clean the build.** Do not run `cargo clean`, do not delete `target/`, and do not pass flags that force a full rebuild.
- Always build incrementally (`cargo build`, `cargo check`, `cargo test` as-is). Full rebuilds of this workspace are very slow.
- If a build looks stale or broken, fix the actual cause — don't reach for a clean build as a shortcut.
