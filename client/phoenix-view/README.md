# phoenix-view

`phoenix-view` runs Discord's real client code in a GPUI window. The code is
the guest compiled into Discord's Android `x86_64` host library
(`liblibdiscord_wasm_host.so`). `phoenix-guest` loads that library natively on
Windows or Linux, and phoenix-view is the platform around it: it applies the
guest's render turns to a tree, lays the tree out, paints it with GPUI,
answers the guest's host calls, and sends the guest the window's clicks and
typing. It is a standalone app, not part of what rekindle ships into the
client.

## You bring the binary

phoenix-view ships no Discord code. It needs three files from a Discord
Android build you supply:

- the `x86_64` host library, `liblibdiscord_wasm_host.so`, from the APK's
  `lib/x86_64/`;
- the base APK, for fonts and drawables;
- the density config splits (`split_config.xxhdpi.apk` and the rest), which
  hold the welcome and login screens' images. A base built with
  `requiredSplitTypes="...base__density"` has no density drawables of its
  own, so without a split the welcome and login screens draw no art.

Extract these from a build you have a right to use. Nothing here downloads or
distributes them.

## Pointing it at the files

```
phoenix-view --so <liblibdiscord_wasm_host.so> \
             --apk <base.apk> \
             --split <split_config.xxhdpi.apk>
```

`--so` and `--apk` also read `PHOENIX_SO` and `PHOENIX_APK`. The config splits
beside the base APK are read with it; `--split` adds one kept elsewhere and
may repeat. With no flags and no environment, phoenix-view falls back to the
author's local paths, so you will pass your own. `--help` lists every option.

Fonts and drawables are read from the APKs at run time and never copied out.

## Building and running

```
cd public/client
cargo build -p phoenix-view
target/debug/phoenix-view --so ... --apk ... --split ...
```

Without `--live` the app never touches the network. The guest's `Networking`
and `WebSocket` calls are refused, and secure storage is never answered, so
the login screen is as far as the guest runs. This needs no account.

`--live` gives the guest the network, through an allowlist that admits only
Discord's own hosts over HTTPS on port 443. Type the account's email and
password into the guest's login screen. The session the guest stores is
sealed with DPAPI in `%LOCALAPPDATA%\phoenix-view\storage.dpapi` (or
`--storage <file>`), so the next `--live` run starts signed in. Delete that
file to sign out.

`--selftest` drives the window off the desktop and checks what it paints,
without synthesising any OS input and without an account. `--live --selftest`
adds a refused host and a login with an address under `.invalid`, which
reaches no real account. Both are what CI runs when the APK is present.

## This is an unofficial client

phoenix-view runs Discord's client code outside Discord's official app.
Logging in with `--live` uses your own account and your own network, and is
your responsibility under Discord's Terms of Service. phoenix-view talks only
to Discord's own hosts, keeps your session sealed on disk, and never
redistributes any of Discord's code or assets. Use an account you are willing
to sign in from an unofficial client with.
