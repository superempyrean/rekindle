This directory holds the test fixtures for
`crates/plugin-host/tests/guest_languages.rs`: small TypeScript plugins (and
one in plain JavaScript, `js-runtime`) that exercise what `rekindle build`
adds to script plugins (the runtime adaptations and build-time checks). They
live here, not with the tests, so they find the toolchain installed for the
example plugins (`npm install` in `plugins/`).
