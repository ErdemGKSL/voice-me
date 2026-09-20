---
name: voice-me
status: final
created: 2026-09-20
updated: 2026-09-20
description: Personal hotkey-triggered voice-cloning utility for Linux/Windows. GPUI Kit (gpui-kit/gpui-component) on GPUI; this DESIGN.md specifies the brand-layer delta only.
colors:
  # gpui-kit / GPUI Component theme tokens inherited wholesale (background, foreground,
  # muted, muted-foreground, popover, popover-foreground, sidebar, border, input,
  # focus-ring, danger, warning, success, info). Only primary and the overlay
  # surface are brand-overridden. Dark theme only for v1. [ASSUMPTION]
  primary: '#7C6AFF'
  primary-foreground: '#FFFFFF'
  overlay-background: '{background}'
  overlay-border: '{border}'
typography:
  # Platform UI font inherited from gpui-kit for all body/label/muted text.
  # Monospace reserved for the hotkey chip and shortcut hints only.
  shortcut:
    fontFamily: 'monospace'
    fontSize: 12px
    fontWeight: '500'
rounded:
  # gpui-kit theme radius inherited for ordinary app chrome (settings window,
  # buttons, inputs). The Prompt Overlay alone uses a rounder radius so it
  # reads as a floating command surface, not app chrome. [ASSUMPTION]
  overlay: 12px
spacing:
  # gpui-kit semantic spacing scale (xxs 2px .. xxl 32px) inherited as-is; no overrides.
components:
  prompt-overlay:
    background: '{colors.overlay-background}'
    border: '{colors.overlay-border}'
    radius: '{rounded.overlay}'
  hotkey-chip:
    background: '{muted}'
    foreground: '{muted-foreground}'
    typography: '{typography.shortcut}'
    radius: '{rounded.sm}'
---

## Brand & Style

voice-me is a personal utility, not a branded product — it exists so its one user can "speak" without speaking, fast, without breaking flow. The visual premise follows: **quiet everywhere except the one moment that matters.** The app is invisible until summoned (tray-only between uses); when summoned, the Prompt Overlay is a calm, dark, focused command surface in the spirit of Raycast and Spotlight — nothing competes with the act of typing a line and hitting Enter.

voice-me inherits gpui-kit / GPUI Component's theme and component defaults wholesale, per the project's own [gpui-kit Design Guides](../../../../.agents/skills/gpui-kit-design-guides/references/design-guides.md), which this DESIGN.md defers to for every rule not overridden here: semantic color roles, the spacing/typography scale, density tiers, state treatment, and component composition. This file specifies only the brand-layer deltas — one accent color and the overlay's own rounder radius. Everything else (Settings window chrome, buttons, inputs, dialogs) uses gpui-kit's defaults as-is.

## Colors

- **Primary Violet (`#7C6AFF`)** `[ASSUMPTION — easy to swap, isolated to this token]` — the one accent. Used for the primary action in Settings (e.g. "Save"), the active/recording indicator during voice capture, and the focus-ring emphasis on the Prompt Overlay's input when it first appears. Evokes "voice/AI" without following an obvious platform convention (not Discord's blurple, not a generic green mic icon).
- **All other tokens** (`background`, `foreground`, `muted`, `muted-foreground`, `border`, `input`, `popover`, `sidebar`, `danger`, `warning`, `success`, `info`, focus-ring) inherit gpui-kit's dark theme defaults unchanged, read from `cx.theme()` by semantic role per the Design Guides — never a raw hex in application code.
- **Dark theme only for v1.** `[ASSUMPTION]` No light theme is designed; if gpui-kit's theme switcher is exposed anywhere, it's out of scope until requested.

Avoid: a second accent color, semantic colors (danger/warning/success) used decoratively, any raw hex/rgb/hsla in application code outside this token layer.

## Typography

Inherits gpui-kit's platform UI font for all interface text — window titles, labels, body, muted metadata. The one addition is a **monospace shortcut style** (12px, medium weight) reserved exclusively for rendering the configured hotkey as a chip (e.g. `⌃⇧Space` / `Ctrl+Shift+Space`) in Settings and in any "press your new hotkey" capture state. Never used for prose.

## Layout & Spacing

gpui-kit's semantic spacing scale (`xxs` 2px through `xxl` 32px) inherited as-is; no product-specific scale. The Prompt Overlay uses `lg`/`xl` internal padding (generous breathing room around a single input, per the "utility window" shell — one focused task, short fixed action path) — Settings uses the ecosystem-default `md`/`lg` rhythm for grouped fields.

## Elevation & Depth

The base app has almost no elevation: it's a tray icon and, occasionally, one window. The Prompt Overlay is the one surface that earns strong elevation — it floats above every other window on the OS, so it gets the strongest shadow treatment gpui-kit's popover/dialog family provides, reinforcing "this is the topmost decision layer right now." Settings uses gpui-kit's ordinary window chrome — flat, no invented shadow tiers.

## Shapes

Rounder than gpui-kit's default app-chrome radius for exactly one surface: the Prompt Overlay (`12px`, vs. gpui-kit's default control radius elsewhere). This is the single deliberate brand accent in the whole shape language — it's what makes the overlay read as "a floating command palette" the instant it appears, distinct from ordinary window chrome. Settings, dialogs, and every other surface use gpui-kit's theme radius unmodified.

## Components

voice-me uses the following gpui-kit components as-is, unchanged: `Button`, `Input`, `Dialog`, `Switch`, `Select`, `Tooltip`, `DropdownMenu` (tray/context menus), `Badge` (dependency/status states). The contract: don't customize these beyond the one primary-color override.

Brand-layer components:

- **Prompt Overlay** — custom composition, not a stock gpui-kit surface (its "borderless, always-on-top, single-line" contract doesn't match any existing component). Built from gpui-kit's popover-family surface treatment (elevation, `overlay-background`, `overlay-border`) plus `rounded.overlay`, holding one `Input` and nothing else. See EXPERIENCE.md for its behavior.
- **Hotkey chip** — small `monospace`/`shortcut` typography badge, `muted` background, used wherever a configured or in-progress hotkey combination is displayed.
- **Recording indicator** — a `primary`-colored dot/waveform accent shown only while capturing a Reference Voice Sample in Voice Setup; the one place `{colors.primary}` appears outside a button.

## Do's and Don'ts

| Do | Don't |
|---|---|
| Inherit gpui-kit defaults for everything not named above | Introduce a second accent color |
| Use Primary Violet only for the primary commit action, the recording indicator, and overlay focus emphasis | Use Primary Violet for chrome, hover states, or decoration |
| Keep the Prompt Overlay to one `Input`, nothing else | Add extra controls, tabs, or a language switcher to the Overlay (out of scope for v1) |
| Reserve the rounder `overlay` radius for the Prompt Overlay alone | Apply the overlay's rounder radius to Settings or dialogs |
| Use the monospace shortcut style only for key combinations | Use monospace for any prose or labels |
