// {{TITLE}}: fetches from an allowlisted HTTPS host.
//
// Every host you contact must be listed in package.json's `rekindle.network`;
// the plugin can reach those and no others, and the install screen lists
// them. Requests never carry credentials.

import { definePlugin } from 'rekindle';
import * as http from 'rekindle/http';
import { Panel } from 'rekindle/ui';

const URL = 'https://httpbin.org/uuid';
let last = '(nothing yet)';

function show(): void {
  new Panel('main', '{{TITLE}}')
    .text(`Last response: ${last}`)
    .button('fetch', 'Fetch')
    .on('fetch', () => {
      try {
        const res = http.get(URL);
        last = res.ok ? res.text() : `HTTP ${res.status}`;
      } catch (e) {
        last = `error: ${e instanceof Error ? e.message : e}`;
      }
      show();
    })
    .show();
}

export default definePlugin({ activate: show });
