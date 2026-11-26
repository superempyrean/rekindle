# rekindle-ui

One plugin UI, two native backends.

A plugin writes a tree from a small semantic vocabulary — row, column, text,
button, toggle, icon, field, surface, separator — and a theme as tokens. The
host lowers that same tree twice:

| Backend | What it produces | Where |
|---|---|---|
| wire | a `rekindle-wire` turn (`Create`/`Children`/`Root`) applied by a split GPUI client | `wire` |
| GPUI | a description of real `Div` and text elements, with `StyleRefinement` field names | `gpui` |

Neither backend draws. The GPUI description is consumed by the desktop backend
(`rekindle-desktop`), which resolves the client build's own `Div` type,
`Drawable<Div>` vtable and `StyleRefinement` offsets at runtime and calls them.
Because the client's layout engine, fonts, spacing and theme do the work, the
result is native rather than a lookalike.

```rust
use rekindle_ui::{Node, Padding, Theme, Ui};

let ui = Ui::new(
    Node::surface(vec![
        Node::heading("Rekindle"),
        Node::text("Rendered by the client's own widgets."),
        Node::row(vec![
            Node::toggle("enabled", "Enable", true),
            Node::button("apply", "Apply").primary(),
        ])
        .gap(8.0),
    ])
    .padding(Padding::all(16.0)),
);

let theme = Theme::dark();
let wire = rekindle_ui::wire::to_turn(&rekindle_ui::wire::lower(&ui, &theme));
let gpui = rekindle_ui::gpui::lower(&ui, &theme);
```

Tokens keep their canonical names (`background-brand`, `text-strong`,
`border-subtle`, ...) on both paths. A token a theme does not set falls back to
the caller's colour, never to a blank surface.

`cargo test -p rekindle-ui` covers the vocabulary, both lowerings and the token
adapter.
