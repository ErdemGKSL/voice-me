---
title: 'Make the Prompt Overlay a pill-shaped prompt bar'
type: 'feature'
created: '2026-09-24'
status: 'done'
route: 'oneshot'
review_loop_iteration: 0
context: []
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** When the hotkey summons the Prompt Overlay, the input sits inside a 12px-rounded card with padding all round it, inside a further transparent inset. It reads as a boxed window holding a field, not a prompt bar popping up.

**Approach:** When the overlay is ready to type into, the overlay *is* the bar. The window is exactly the bar's size, with no card or padding around it. The bar is a pill (fully rounded ends, theme-aware) and holds, left to right: a muted voice icon, the one `Input` (same placeholder, text size and behavior), and a quiet Enter key hint. The bar's edge takes the focus-ring colour while the input is focused, which is DESIGN.md's "overlay focus emphasis". The summon and dismiss animations fade only, since there is no inset to animate.

**Decisions (from the user):** pill shape, not a rounded rectangle. Scope is the typing bar only: the blocked ("can't speak yet") and confirm-first (disclosure) shapes keep their current look, size and behavior. Enter speaks, Escape and focus loss dismiss, and keyboard-only operation is unchanged.

</frozen-after-approval>

## Implementation Notes

- `crates/voice-me-ui/src/prompt_overlay.rs`: `render` matches on (disclosure, blocker). The prompt shape draws `prompt_bar`, a pill (`rounded_full_style`) of its own `PROMPT_BAR_HEIGHT` (56px). It holds a muted `AudioLines` icon, the `Input` (unchanged: 16px text, placeholder, no appearance), and a muted, borderless `Kbd` for Enter, which is "Enter" or ⏎ per platform. The edge is `theme.border`, or `theme.ring` while the input is focused. It has no shadow: the bar fills its window, so a shadow would be clipped into the corners around the rounded ends. It fades in and out with no inset. The blocked and disclosure shapes are the old card, now built by `card(body)` with `blocked_body` / `disclosure_body`, so no shape can reach a panic path. `confirm()` asks the window to shrink to the bar's height, since the card needed 176–196px, and the bar keeps its own height if a compositor ignores that.
- `crates/voice-me-app/src/main.rs`: the window is `PROMPT_BAR_HEIGHT` tall for the prompt shape; the blocked card keeps 84px (`OVERLAY_BLOCKED_HEIGHT`) and the disclosure 196px. `AppAssets` layers `icon_assets!(PromptBarIcons, [AudioLines])` over gpui-kit's default `Assets`, instead of embedding the whole Lucide catalog (about 7 MB) for one icon.
- `DESIGN.md` updated: the `prompt-bar` component, radius and spacing, and the Shapes and Components prose.
- Not verified visually: this container has no Vulkan driver, so the overlay cannot open here. Verified with 123 `voice-me-ui` tests (including bar geometry, the bar after confirming, and focus kept when clicking the bar) and an `AppAssets` test.

## Spec Change Log

- User report after release (screenshot): the bar ran off the bottom of a window whose client area came out much shorter than 56px, and a white square showed behind the pill's rounded ends. Amended: the overlay's `Root` is transparent (`PromptOverlayView::root`), since `Root` fills its window with the theme background. The bar now fills the window up to `PROMPT_BAR_HEIGHT` instead of a fixed height, so it never overflows. Why that window's client area is so short is not settled yet. KEEP: the bar is capped at its own height, so a taller confirm-first window never stretches it.

## Review Triage Log

- high (patched): after confirm-first, the 196px window made the bar a tall pill. `confirm()` now resizes, the bar has its own height, and there is a test.
- medium (patched): no test covered confirm-first turning into the bar. Added to the Enter-confirms test.
- medium (patched): `AllAssets` embedded about 7 MB of icons for one. Now `icon_assets!` over `Assets`, with a load test.
- low (patched): `body()` had an `unreachable!` path. `render` now matches on the shape.
- low (rejected): no test for the focus-border colour. Colour assertions need scene inspection, and the focus behaviour itself is covered.
- medium (deferred): DESIGN.md's violet focus emphasis is never applied to the theme. Predates this change.
- low (rejected): the edge changes weight between card and bar after confirming. It happens once per provider and is cosmetic.
- low (patched): the corner-click test clicked outside the rounded bar. It now clicks the icon.
- medium (deferred for epics.md/EXPERIENCE.md, patched for the module doc): documents described "one Input and nothing else".
- false/low: DESIGN.md token formats. Inherited tokens are written `'{name}'` (as `'{border}'` is); added `shadow: none`.
- medium (patched): the Enter hint was a full bordered badge. It is now muted and borderless. It always shows, as the discoverable Enter hint.
- false: "the spec is incomplete". The one-shot route fills Implementation Notes at the end, as here.

