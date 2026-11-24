# Theme playground

The playground shows a theme on the client's real screens while you write
it. Pick a screen, change a token's colour, and the screen is recoloured as
you drag. Export the result as a `.theme.json`.

The screens are wire captures: the turns Discord's client sent to build
them, replayed in the browser. The recolouring is rekindle's wire engine
(`crates/wire-inject`) compiled to WebAssembly, the same code the host runs
on the client's wire, so a colour the playground shows is the colour the
host writes.

## Running it

```
cargo run -p rekindle-playground -- serve
```

This builds the wasm module (`cargo build --target wasm32-unknown-unknown`)
and serves the page on <http://127.0.0.1:7780/>. `--port` picks another
port, and `--no-build` serves the module built last.

## The client's icons and fonts

The fixtures name the client's drawables (`res://ic_mana_gear`) and fonts,
which are Discord's and aren't in this repository. Without them, an icon is
drawn as a box in its tint and text uses a system font. If you have the
client's Android APK, phoenix-view extracts both into `playground/local/`,
which git ignores:

```
cd client
cargo run --release -p phoenix-view -- --apk <base.apk> \
    --export-assets ../playground/local \
    --from ../playground/fixtures/chat.capture --from ...
```

Pass every capture you will open with `--from`, so each drawable it names
is exported. The density splits beside the base APK are read too, as
phoenix-view reads them.

## Screens

`fixtures/index.json` lists the screens. Each is one capture of Discord's
Android client, build 356207, cut down to a single turn that creates the
screen as the client last showed it. Every name, message, server name and
image was replaced by `rekindle-playground sanitize`. The client's own
labels on `fixtures/keep.txt` were kept.

A screen also names the palette the guest had named when it was captured.
The binding (`fixtures/binding.json`) reads each token's colour from that
palette, and a token is recoloured wherever the screen uses that colour in
a cell of the token's role. For now, only the two new-server screens have
a logged palette. The others carry a modelled one, so a token there can
miss cells it would reach on the client.

To try a capture of your own, use "Open a capture", or list it in
`local/index.json` (the same shape as `fixtures/index.json`, with paths
relative to `local/`). To make a fixture out of one:

```
rekindle-playground strings my.capture          # every string it holds, by cell
rekindle-playground sanitize my.capture fixtures/my.capture --keep fixtures/keep.txt
```

Read the output of `strings` on the sanitized file before committing it.

## How close it is

Layout is phoenix-view's (`client/phoenix-view/src/view.rs`), written for
CSS flexbox. Sizes, padding, margins, gaps, radii, borders, clipping,
gradients, shadows, stacks, navigators, lists and text spans follow the
cells. Text is laid out by the browser with the client's fonts, so a line
can wrap a word earlier or later than on the device. Remote images (avatars,
attachments) are grey boxes, and pressed and animated states aren't drawn:
the screen is shown at rest. Cells that phoenix-view doesn't read
(`activeBackground`, `hairline`, `borderLeft`) aren't drawn here either.

The colours are exact. They come from the engine's tree, and
`playground/src/tests.rs` checks that a theme applied to a screen gives the
same tree as an engine that had the theme before the guest's first turn,
which is how the host runs.

## The URL

`?screen=<id>&view=split&theme=<url>&variant=<name>&turn=<n>` opens the page
on a screen, a view (`themed`, `original` or `split`), a theme file and a
turn of the capture.
