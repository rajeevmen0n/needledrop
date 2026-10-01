---
version: alpha
colors:
  background: '#000000'
  surface: '#151515'
  primary: '#D8B983'
  text: '#F3EFE7'
  muted: '#96999E'
  border: '#303030'
  controlBorder: '#666666'
typography:
  display:
    fontFamily: "'Bricolage Grotesque Variable', system-ui, sans-serif"
    fontWeight: 800
    lineHeight: '0.98'
  body:
    fontFamily: "'Bricolage Grotesque Variable', system-ui, sans-serif"
    fontSize: '1rem'
    lineHeight: '1.5'
rounded:
  control: '6px'
  disc: '999px'
spacing:
  small: '0.5rem'
  medium: '1rem'
  large: '1.5rem'
  section: '2rem'
components:
  button:
    height: '3.5rem'
    backgroundColor: '{colors.primary}'
    textColor: '{colors.background}'
    rounded: '{rounded.control}'
  input:
    height: '3.5rem'
    backgroundColor: '{colors.surface}'
    textColor: '{colors.text}'
    rounded: '{rounded.control}'
  secondary:
    textColor: '{colors.muted}'
  separator:
    backgroundColor: '{colors.border}'
  controlBoundary:
    backgroundColor: '{colors.controlBorder}'
---

# Needledrop

## Overview

A midnight listening room: the quiet precision of a hi-fi turntable, one unknown
record, and a familiar feeling waiting to be recognised. This daily music game is
a hybrid surface: the vinyl carries the brand expression, while controls remain
plain, legible, and familiar. The owner approved this direction as a replacement
for the former saturated weekday sleeves.

The memorable object is a fully visible, finely grooved vinyl record with a
machined silver tonearm and a custom ivory test-pressing label. Restrained
champagne details lead the eye toward listening and guessing. Avoid dashboard
cards, neon audio visualisers, bright rings, decorative pills, and crowded
turntable controls.

## Colors

One constant dark theme, independent of weekday or system preference. OLED black
is the page; graphite provides the search and popup surfaces. Ivory carries
primary text, silver secondary text, and champagne identifies the primary
action and unlocked clips. State always has words or shapes as well as color.

Runtime ownership is manual CSS: `web/src/styles/tokens.css` is the sole token
implementation. Background maps to `--field`, surface to `--surface`, primary
to `--accent`, text to `--paper`, muted to `--muted`, and border to
`--border`. The canvas consumes these CSS values through Record.svelte.
The dark `--ink` token is foreground on champagne and ivory.
Material highlights inside the record painter are expressive illustration
colors, not additional UI semantic tokens.

Control boundaries map to `--control-border` (#666666), keeping the search field
boundary above 3:1 against graphite and black. Decorative dividers retain the
quieter `--border` value.

## Typography

Self-hosted Bricolage Grotesque throughout. Regular-width semibold type sets the
question; condensed heavy display type sets the revealed song title.
Regular-width body type keeps labels and history readable.
Use tabular numerals for clip lengths and the pressing number.
The compact wordmark and daily pressing are deliberately subordinate to the
record and question. Keep the question short and precise.

English interface, UTC game date, and UTC reset. The font falls back to system
sans-serif for unsupported characters. Song metadata can contain any script;
wrap the reveal and attempt history and truncate autocomplete rows only.

## Layout

Desktop: header across the page, a fully visible record on the left, a focused
game column on the right, and a quiet footer. At 40rem with landscape aspect
ratio, switch to two columns; other viewports stack the record over the controls.
Document scrolling owns overflow; never lock the game to a fixed viewport.

On phones, reduce the headline and record to keep play, search, and skip nearby.
Safe-area gutters, 44px minimum targets, 56px main controls, and 16px input text
support touch. Seven equal clip segments communicate the ladder at a glance;
history shows used and current attempts without seven empty visual rows.

## Elevation & Depth

Static content sits directly on black. Borders quietly establish structure.
The search popup alone gets a restrained dark shadow. Reflections, groove
hairlines, bearing details, and layered metal highlights give the canvas physical
depth; keep this realism concentrated in the record.

## Shapes

Circles belong to vinyl, label, spindle, and play. Inputs and rectangular buttons
have 6px corners. No generic rounded card containers. Use fine strokes and
precise simple icons, with an arrow for the secondary skip action.

## Components

Shared owners: Play is used both before and after the reveal; GuessInput owns
autocomplete and its authored listbox; Skip owns progression; Attempts owns
readable history; Record owns animation and the canvas painter. API and game
state remain the established behavioral authority in AGENTS.md.

Search is transient game input, deliberately absent from URL state. It has a
clear button, debounced requests, cancellation, IME safety, keyboard selection,
and viewport-aware popup placement. Guess remains unavailable until a result
is selected. Failed guesses retain their selection for retry.

Loading, search failures, no results, playback errors, and move errors use the
existing app-owned inline feedback and live regions. Disabled controls stay
fixed in size. Global scrollbars live in base.css and include hover, active,
and forced-colors behavior.

Playback drives rotation and stylus movement. The disc coasts after sound stops;
the reveal prints album artwork on the label and settles upright. Reduced
motion suppresses rotation, groove response, and reveal transitions while
keeping game state and playback progress available.

## Do's and Don'ts

- Keep true black and ample empty space around the record.
- Make metadata, keyboard focus, and primary actions readable.
- Let the real object do the visual work; avoid extra ornamental widgets.
- Keep the server authoritative and never reveal answer data while playing.
- Preserve the listening and guessing order across viewports.
- Verify sound quality by ear with the owner; a headless browser cannot do that.
