# rekindle-desktop

Desktop Phoenix is a single native Rust/GPUI binary. There is no wire and no
webview to draw over, so rekindle's desktop UI has to be made of the client's
own elements. This crate finds them in a specific `Discord.exe` at runtime.

## What it resolves

`Symbols::resolve` reads the image file (never the live process) and finds:

- **`MessageRow::render`** from the unique `\x10message-content-\xc0\x00`
  format template and the eight-push Rust prologue. The build has two
  row-shaped renderers; both are reported.
- **`Drawable<Div>`** from a 264-byte `dyn ElementObject` vtable whose drop
  glue drops a `Div` at `+0x80`; that drop frees the boxed `Interactivity`
  (`0x4e8` including the `Rc` header) and the children `SmallVec` at `+0x40`.
- **`StyleRefinement` field offsets** from the build's own derived
  `Debug::fmt`: the `lea` to each field name, the `mov` of its length, the
  `lea` to the field inside `self`, and the `DebugStruct::field` call. Nested
  structs (`padding`, `gap`, `corner_radii`, `size`, ...) are followed through
  the field's `&dyn Debug` vtable at `+0x18`. The struct size is derived from
  the last field, not carried as a constant.

Nothing is a fixed RVA. A build whose shapes changed fails before anything is
written.

## Check a build

```powershell
cargo run -p rekindle-desktop -- check `
    --exe "$env:LOCALAPPDATA\DiscordPhoenix\app\Discord.exe.orig"
```

Against build `discord-desktop-phoenix@0.1.0-dev.20261123093910.gb71ec80`
this prints:

```
MessageRow::render 0x1446705fd (RVA 0x46705fd)
MessageRow::render 0x144be619b (RVA 0x4be619b)
Drawable<Div> 0x15643a9c0 (RVA 0x1643a9c0); 4 candidate(s)
StyleRefinement 640 bytes
  background               +0x150
  padding                  +0x198
  gap                      +0x1b8
  corner_radii             +0x130
  border_color             +0x250
  opacity                  +0x100
  ...
check: ok (44 fields, StyleRefinement 640 bytes)
```

`--json` writes the same report for tooling; a release's CI keeps the JSON
beside the anchors it validates.

## The message context menu

`Menu::resolve` finds `MessageContextMenu` and the `Vec<MenuItem>` it builds:
`Vec<MenuItem>::push`, the `0x1F8` item stride, `MenuItemProps` at `0x20`, the
visible label `SharedString` at `0xB0` (`0x8018`, pointer at `0xB8`, length at
`0xC0`), `Action` tag `2`, and the separator tag.

```
$ rekindle-desktop menu --exe ...\Discord.exe.orig
MessageContextMenu 0x142816d4c (RVA 0x2816d4c)
Vec<MenuItem>::push 0x142814c4e (RVA 0x2814c4e)
item 0x1f8 bytes, props at 0x20 (0x1c8 bytes)
label at +0xb0 (tag), +0xb8 (ptr), +0xc0 (len); Action tag 2
```

On a disposable client, rewriting that `SharedString` on the returned Vec
renders **rekindle** in place of the Reply row. The label is the client's own
24-byte static `SharedString` and owns nothing, so the menu's normal drop is
safe. A native detour can also build a new `MenuItem::Action` with
`MenuItemProps::new` and `with_id`, keep the Reply row's handler, and append
the old row with `Vec::push`; the captured menu then has the new row at
Reply's position and `len + 1` rows. The resolver locates every builder.

## The request function

`typing::resolve` finds the function every REST request goes through, from
the client's typing indicator. Each place that sends `POST
/channels/{id}/typing` loads a format template holding `/channels/` and
`/typing`, formats the path, and passes it to that function with the method:
`(client, &method, path pointer, path length, a, b)`. It returns the
request's boxed future in `rax:rdx`, and only the future reaches the network
once it is polled. The resolver takes the first call after each site's
`format` that every site makes and whose entry stores `r8` and `r9`.

`typing::is_typing_path` says whether a path is a typing request's.

A platform running inside the client resolves it from the live module with
`Image::mapped`, which reads the image in its loaded layout instead of the
file's.

## Writing a style

`native::write_style` writes a `rekindle_ui::GpuiStyle` into a live
`StyleRefinement` at the resolved offsets. The encodings are gpui-pre 0.3.14's:
`Option<Fill>` is a 72-byte solid fill (tag 0, colour space 0, `Hsla` at `+8`),
`Option<Hsla>` is `{1, Hsla}`, a length is `{tag, f32}` (0 px, 2 fraction,
`u32::MAX` auto), an absolute length is `{0, f32}`, and `flex_grow`/`flex_shrink`
are plain `f32`. A field a build does not name is skipped and reported, never
guessed.

## What is not done yet

`div()` and `<Div as IntoElement>::into_any_element` are inlined on this
build, so there is no standalone constructor to call. The resolver finds the
`Drawable<Div>` vtable and the exact `Div`/`Interactivity`/`StyleRefinement`
layout; the remaining step is to splice a newly built `Div` into a surface
whose `render` returns an `AnyElement` we can replace. Probing the composer
and message row on the disposable client has not yet found that splice point,
because both return custom element structs rather than `AnyElement`.

What is proven live is narrower: the message context menu can carry a new
`MenuItem::Action` built with the client's own `MenuItemProps::new`/
`with_id` and appended with `Vec::push`; the captured menu shows it at
Reply's position with the old Reply kept at the end. Building and placing a
`Div` remains open.
