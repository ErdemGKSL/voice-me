# Glossary — voice-me

Terms the SPEC and its companions use verbatim; no synonyms.

- **Reference Voice Sample** — the short audio clip of the user's own voice, recorded or imported once, used to clone their voice for every generated line (CAP-1, CAP-5).
- **Prompt Overlay** — the small, borderless, always-on-top single-line text input that appears on hotkey press and closes on Enter or Escape (CAP-3, CAP-4).
- **Speak Action** — the act of the Prompt Overlay closing on Enter, triggering TTS generation and playback for the typed line (CAP-4, CAP-5, CAP-6).
- **Virtual Microphone** — an OS-level virtual audio input device that other applications (games, Discord, Zoom) can select as their microphone; voice-me plays generated audio into it instead of a physical mic (CAP-6).
- **Sidecar Process** — the local background process hosting the TTS engine's Python/PyTorch runtime, managed and provisioned automatically by the main app (CAP-5, CAP-7; see Architecture Spine for its adapter, `voice-me-tts`).
- **Dependency Check** — the app's scan for locally missing components the Sidecar Process needs, surfaced in-app with one-click provisioning rather than a setup wizard (CAP-7).
- **Preset Phrase** — (non-goal for v1) a fixed phrase bound directly to a hotkey, spoken with no Prompt Overlay shown at all.
