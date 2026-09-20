# The console's design system

`src/server/handlers/console/assets/app.css` is built on scales, not on values
chosen per rule. There is no linter for it, so the scales hold only if each
change is checked against them: **a length, size, radius or colour that is not
one of the tokens below is a regression, not a choice.**

## Tokens

Declared at the top of `app.css` under `:root`, and again under
`@media (prefers-color-scheme: dark)` for the colours alone — the scales do not
change with the theme.

| Scale | Values |
| --- | --- |
| Space | `--space-1` … `--space-6`: 4, 8, 12, 16, 24, 32 |
| Radius | `--radius-sm` 4, `--radius-md` 6, `--radius-lg` 10, `--radius-pill` 999 |
| Type | `--text-2xs` … `--text-xl`: 11, 12, 13, 14, 15, 20 |
| Leading | `--leading-tight` 1.25, `--leading-normal` 1.45 |
| Control height | `--control-h` 30, `--control-h-sm` 24 |
| Elevation | `--elev-1`, `--elev-2` |

Colour comes in named roles, never as a literal: surfaces (`--bg`,
`--surface`, `--surface-hover`, `--sunken`), text (`--text`, `--muted`,
`--faint`), lines (`--line`, `--line-strong`), and four judgement families
each with a foreground, a soft fill and a line — `--accent`, `--ok`,
`--warn`, `--err`. Charts draw from `--series-1…3` and `--critical`, which
were validated as a set against `--surface`.

A number that is not on a scale usually means the rule is drawing something
the components already draw. Reach for the component first.

## Badges

Four shapes, and the taxonomy is the point.

| Shape | What it is for | How it looks |
| --- | --- | --- |
| `state` | what something is doing or how it ended | pill, dot, one of the judgement colours |
| `http` | a status code | square, mono |
| `kind` | a classification | neutral, no colour |
| `tag` | an annotation on what a client sent | dashed border |

**Colour means judgement.** A classification never gets one: that is why
`kind` is neutral, and why a new badge that merely names a category must be
`kind` rather than a fifth colour.

The session is the one standing exception, and it stays one. A session is a
classification, but the console's whole job on the Requests and Recordings
tabs is to let a reader see which rows belong together, which no amount of
neutral styling does. So a session carries a hue — on the rail down a row's
first cell, and on the chip in a detail panel — and nowhere else. Because the
hue is an exception, it never carries the identity alone: the session's number
is drawn beside it, in the plain small print of the line it sits in, for a
reader who cannot see the colour and for the moment the six-hue palette wraps
around and two sessions share one.

## Rules the file already keeps

- **No inline style.** The page's CSP forbids it; a hue is a class
  (`hue-252`), not a `style` attribute.
- **Both themes, every time.** A colour added for light mode without its dark
  counterpart is a half-written rule.
- **`title` is not documentation.** It is for the value a cell had to shorten,
  such as a session id under its number.

## Not yet applied

From the design canvas that produced the current `app.css`: the Overview's
Router cards (still six equal tiles, three of which repeat what the header
already says, with long paths wrapping), the redacted configuration shown as
JSON rather than the TOML the user actually edits, sortable numeric columns,
and filter and range state in the URL.

## Checking a change

`cargo test` does not see any of this: the assets are served as files and the
tests are black-box HTTP.

**Read the diff against this page first.** It is the cheapest check and the
one most often skipped, and both halves of it were broken repeatedly while
the session rail and the model comparison were written:

- **Every length, size, radius and colour is a token.** A `3px` rail, a `6px`
  margin, a `10px/14px` font: each was written, none survived review. If a
  value is not on a scale above, the rule is drawing something the components
  already draw.
- **Every control is an existing component before it is a new one.** A button
  that reads as text is `button.link`, not a fresh set of resets — the fresh
  set missed the height the component already handles, and the line it sat in
  grew by 13px. Check the component inventory before adding to it.
- **Colour still means judgement.** A new badge that names a category is
  `kind`. The session is the one exception and stays one.

**Then run `scripts/console-check`, and read the screenshots it took.**

    scripts/console-check --token <server.token> --shots shots/

It reads the router named by `--url` or `CONSOLE_CHECK_URL`, defaulting to
the example configuration's `127.0.0.1:8787`. Give the router it drives a
port of its own: a check that restarts it, or fills it with requests to have
something to look at, is not something to do to the router a session is
using.

It drives a running router across every tab, both themes and seven widths,
and reports what it found with the screenshot each finding came from. What it
reports is a hint, not a verdict: it can say that something is 30px tall in a
17.4px line, that a panel overflows its box, that two blocks have nothing
between them, that a page threw. It cannot say whether a page reads well, and
several of the numbers it prints — a thirteen-column table scrolling inside
its wrapper at 900px — are the console working as designed.

**The screenshot decides.** Open the ones it names, and open the rest anyway:
a clean run is not a passed review. Look at the page the change touched, in
both themes, at a width someone actually uses.

**A number that was already there is not a regression.** Take a reading
before the change, and hold the one after it against it:

    git stash && cargo build && scripts/console-check --token … --out before.json
    git stash pop && cargo build && scripts/console-check --token … --baseline before.json

**Some things it does not reach**, and that a change touching them has to be
driven for by hand: hover, focus and selected states; a panel that redraws on
a poll (frames over 50ms, whether what the reader opened is still open);
anything behind an interaction, such as the model comparison, which is only
drawn once rows are picked.
