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

The record hangs in a quiet night sky: stars, a little haze, and light from
behind the disc. The owner asked for this on 2026-10-01 in place of the floating
notes and groove trails ("space vibes", tasteful, not annoying). It is
atmosphere, not a second subject: nothing in it may compete with the record or
the controls.

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

Champagne is for playing: the play control, Guess, random mode's Next song,
unlocked clips, the selected tab's line, a win, today's bar in the stats. It is never the colour of a
destructive action. The button that confirms a deletion is outlined in ivory,
and there is no red: a problem is ivory text with the "!" mark.

Control boundaries map to `--control-border` (#666666), keeping the search field
boundary above 3:1 against graphite and black. Decorative dividers retain the
quieter `--border` value.

The sky has its own decorative tokens, never used for text or a control
boundary. `--sky-warm` (`216 185 131`, the accent as colour channels) is the
light behind the record, the warm haze and the glow of the primary controls.
`--sky-cool` (`58 86 156`, one deep blue) is the cool haze. Both are channels
so that each use sets its own low alpha: at most 0.3 at the record's edge and
0.17 in the haze, so the field still reads as black. `--star-cool` (#C7D2E8) is
the few pale blue stars; the rest reuse `--paper`, `--muted` and `--accent`.
`--accent-bright` (#E8D2A6) is a champagne control lit by hover; dark ink on it
has more contrast than on the accent itself.

## Typography

Self-hosted Bricolage Grotesque throughout. Regular-width semibold type sets the
question; condensed heavy display type sets the revealed song title.
Regular-width body type keeps labels and history readable.
Use tabular numerals for clip lengths and the pressing number.
The compact wordmark and daily pressing are deliberately subordinate to the
record and question. Keep the question short and precise. On the Random tab
the pressing is numbered by its place in the session ("Random pressing ·
Song 4") in the header and on the record's label, where the daily tabs carry
the day.

English interface, UTC game date, and UTC reset. The font falls back to system
sans-serif for unsupported characters. Song metadata can contain any script;
wrap the reveal and attempt history and truncate autocomplete rows only.

## Layout

Desktop: header across the page, the row of section tabs under it, a fully
visible record on the left, a focused game column on the right, and a quiet
footer. At 40rem with landscape aspect ratio, switch to two columns; other
viewports stack the record over the controls. The game shell fits the visible
viewport, keeping the header, tabs, record and footer in place as sections
change. The game column owns overflow for long reveals, history, short screens
and zoom; switching sections returns that column to its top. The panel fades
in without moving the record. Keep every control and song detail reachable.

On phones, reduce the record and omit the introductory headline to keep play,
search, and skip nearby.
Safe-area gutters, 44px minimum targets, 56px main controls, and 16px input text
support touch. Seven equal clip segments communicate the ladder at a glance;
history shows used and current attempts without seven empty visual rows.

The four sections (General, Pop, Rock, Hip-hop) and random mode (Random, added
at the owner's request on 2026-10-01) are tabs between two hairlines under the
header: five equal columns on a phone, their own width at the left from 40rem.
They must fit a 320px screen at 44px tall, so a tab is one short word and one
14px mark; below 40rem the mark sits above the word, so that the longest word
has a whole column, and from 40rem it sits beside it. The record, the rail, the
controls and the history below always belong to the selected tab, and each tab
has its own address.

A finished section shows its stats between the result and the history: four
figures on one line parted by hairlines, then "Wins by try". No boxes, no
tiles. While a game is open only the streak shows, in the history's heading.

Random mode is the same game without a day: one song after another for as long
as the player likes. While a song is open it looks like a section's game, with
the session's run ("Run: 3" / "No run yet") where the streak would be. Once the
song is over, Next song comes straight after the revealed title and artist in
the stacked layouts, so a phone reaches it without scrolling past the cover;
in the side layout it stands beside the play control of the reveal. Either
way it is before the score and the history. The score is the same line of
four figures (run, best ever, played, won) and has no bars: only the longest
run is kept beyond the session.

The admin page at `/admin` is a tool for one person and is plain on purpose:
the same black, type and tokens, with no sky, no record and no sound. One
column, at most 60rem wide, of panels that are a heading over a hairline, never
a card. Its buttons are the 44px outlined ones of the confirmation dialog, and
"Add to the pool" is its only champagne button. Numbers to read (the clock, the
pool's totals) are text, not controls. It never lists the song pool: songs
appear only as the results of a search, 25 at most, with a line saying how many
more match.

## Elevation & Depth

Static content sits directly on black. Borders quietly establish structure.
Only what floats gets a restrained dark shadow: the search popup, and the
confirmation dialog over a 72% black backdrop. Reflections, groove
hairlines, bearing details, and layered metal highlights give the canvas physical
depth; keep this realism concentrated in the record.

The record hangs in a night sky. Atmosphere.svelte owns one viewport-fixed
canvas beneath the page; `lib/starfield.ts` paints it and `lib/sky.ts` holds the
model. There are 60 to 280 stars by viewport area, the same sky on every visit,
at three depths: mostly tiny ivory and silver points, a few pale blue, and a
handful of larger champagne or ivory stars with a soft halo and faint four-point
spikes. Each star twinkles on its own 3 to 8 second cycle. The sky drifts
westward at 0.35 to 1.3 pixels a second, slow enough to miss while reading.
While a clip plays it eases up to five times that pace and brightens a little,
breathing with the level of the sound, then eases back; it never jumps. Near
stars follow page scroll slightly more than far ones.

A thin champagne shooting star crosses the upper sky every 9 to 22 seconds
(about 5 to 12 while a clip plays) and lasts 0.7 to 1.1 seconds. Stars dim to a
fifth of their light behind anything marked `data-sky-calm` (header text, game
column, record caption, footer), and shooting stars never cross those areas, so
text contrast is untouched. Mark any new block of text the same way.

Two hazes sit on the same layer as plain CSS gradients: warm champagne behind
the record and deep blue in the opposite corner, drifting over 68 to 84 seconds
on the compositor. The light behind the record (`.stage::before` in App.svelte)
is brightest at the edge of the disc and gone within about a radius: an eclipse,
not a ring. It is subtle at idle and a little stronger while a clip plays.

Winning on this page, not reloading a won game, releases stardust from behind
the record's edge and a brief flare over about 1.6 seconds, followed by one
brighter shooting star. A loss adds nothing to the reveal.

The page arrives once on load: sky, header, the record rising with its light,
the game column, the footer. The current step of the clip rail takes a glint as
it unlocks and then a slow breathing glow. Play and Guess gain a soft champagne
glow on hover, Guess one sweep of light, and the play disc opens its glow while
the clip sounds. Nothing bounces.

The footer offers Pause motion / Resume motion, which holds the sky, the haze
and the rail's glow. A hidden tab stops the loop. Reduced motion draws one
static sky, skips the arrival, drift, twinkle, shooting stars and stardust, and
hides the unnecessary pause control. Keep the field black; avoid an audio
equaliser, saturated nebulae, and anything quick enough to notice while typing.

## Shapes

Circles belong to vinyl, label, spindle, and play. Inputs and rectangular buttons
have 6px corners. No generic rounded card containers. Use fine strokes and
precise simple icons, with an arrow for the secondary skip action. The header
mark is a small champagne record with a four-point glint on its rim.

A tab's mark is the state of its game, in the shapes the history already uses:
a ring for not played, a ring with a champagne dot for in progress, a check on
a champagne disc for won, a cross for lost, a dash for no song today. Random's
mark never changes: a loop without an end, in the tab's own colour, with the
word "endless" for a screen reader and the tooltip. It is never won or lost. The
selected tab is marked by a 2px champagne line on the row's rule, not by a
filled shape or a pill.

Bars in the stats are thin (10px), square at the baseline and rounded 4px at
the end the count sits at. Grey bars are context; the one champagne bar is
today's win, and the word "Today" says so as well.

## Components

Shared owners: Play is used both before and after the reveal; GuessInput owns
autocomplete and its authored listbox; Skip owns progression; Attempts owns
readable history; Record owns animation and the canvas painter; SectionTabs
owns the tabs and their keyboard pattern; Figures owns the line of figures,
Stats a section's record around it with the bars, Score a random session's;
Confirm owns the one dialog pattern, for anything that cannot be undone. API
and game state remain the established behavioral authority in AGENTS.md.

Tabs follow the WAI-ARIA pattern with manual activation: the arrows, Home and
End move the focus, Enter or Space opens the tab, because opening one stops the
clip and may wait for the server. Each tab is a link to its own address. Its
state is in its accessible name ("Pop, won"), never in the mark alone.

Numbers drawn large or as bars are decoration to a screen reader; each has a
sentence beside it ("Current streak: 3 days", "Try 3: 4 wins, including
today's"). Text beside a bar wears text colours, never the bar's.

A confirmation is a modal dialog on the graphite surface: the question as its
heading, what will happen in a sentence or two, Cancel first and focused, then
the action named in full ("Clear my data", not "OK"). A failure is reported
inside it and leaves the page untouched. Never use the browser's own confirm.

A section without a song says "No song today" where the game would be, and
offers to check again; random mode with nothing to draw says "No song to
play" the same way. Next song is a primary button that keeps its width while
the next song is found ("Finding…") and cannot be pressed twice; when the song
arrives the keyboard focus goes to the play control, never to the search
field, which would raise a phone's keyboard. News the player did not cause (the song was replaced,
the day changed) is one ivory line with a champagne rule at its left, above
the game, and is announced once.

SongCombobox owns the song search drop-down for the game and for the admin
page alike; GuessInput is the game's form around it. On the admin page a row
may carry a second line (artist and album) and a short outlined mark at its end
("No preview", "In the pool"); a row that cannot be used is dimmed as well, and
the mark says why in words.

A genre tag is a tick box with its name: an ivory tick on graphite, never
champagne, saved on the click and shown as "Saving…" beside the boxes. A
result of an action ("Added …", "Pop now plays …", "Removed …") is the news
line: ivory, with the champagne rule at its left.

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
- Keep the sky quieter than the record: if a star, haze or streak draws the eye
  away from listening and guessing, turn it down.
- Make metadata, keyboard focus, and primary actions readable.
- Let the real object do the visual work; avoid extra ornamental widgets.
- Keep the server authoritative and never reveal answer data while playing.
- Preserve the listening and guessing order across viewports.
- Keep the tab row to one line of five at every width; do not add a sixth
  control to it.
- Do not put the stats in cards or tiles, and do not colour more than one bar.
- Do not list the song pool on the admin page, by tabs, pages or "show more":
  the owner asked for a search box and its results only.
- Verify sound quality by ear with the owner; a headless browser cannot do that.
