// {{TITLE}}: a rekindle plugin.
//
// Handlers (all optional): activate, deactivate, onSettingsChanged,
// onFeaturesChanged, onEvents/onEvent, onUi, onCommand, onSlot, onKeybind,
// onSlashCommand, onAutocomplete, onTimer, onCompose, onUpload, render,
// onNotification, onIntercept. `definePlugin` types their parameters.
//
// The SDK is `rekindle` plus a module per area, named after the contract
// interface it uses: `rekindle/state`, `rekindle/users`, `rekindle/messages`,
// `rekindle/ui`, `rekindle/slots`, `rekindle/decorations`, `rekindle/commands`,
// `rekindle/render`, `rekindle/local-actions`, `rekindle/reactions`,
// `rekindle/theme`, `rekindle/storage`, `rekindle/http`, ... Most need their
// capability in package.json's `rekindle.capabilities` (the build tells you
// which); `rekindle/settings`, `rekindle/events`, `rekindle/timer`,
// `rekindle/time` and `rekindle/elements` need none. User-facing options go
// in `rekindle.settings`, a schema the client renders as a settings page.
//
// Top-level code runs at build time (the build snapshots it), so call the
// client from handlers, starting with `activate`.

import { definePlugin, log } from 'rekindle';

export default definePlugin({
  // Runs once, after load. `granted` lists the capabilities in the manifest.
  activate(granted) {
    // `log.*` and `console.*` both go to the client's plugin log.
    log.info(`{{TITLE}} is up; granted: ${granted.join(', ') || 'nothing'}`);
  },
});
