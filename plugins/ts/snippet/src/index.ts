// Snippet: `/snippet code: … language: …` sends the code as a file
// attachment (`snippet.rs`, `snippet.py`, …) in the channel the command was
// run in, so it doesn't fill the chat and Discord shows it with a preview.
//
// Sending is an `act` action: the slash command is the user's gesture.

import { definePlugin, RekindleError } from 'rekindle';
import { file, sendMessage } from 'rekindle/actions';
import * as commands from 'rekindle/commands';
import { SlashCommand } from 'rekindle/commands';

/** File extensions by language name; anything else is sent as `.txt`. */
const EXTENSIONS: Record<string, string> = {
  rust: 'rs', typescript: 'ts', javascript: 'js', python: 'py', c: 'c', cpp: 'cpp', csharp: 'cs', go: 'go',
  java: 'java', kotlin: 'kt', swift: 'swift', ruby: 'rb', lua: 'lua', shell: 'sh', json: 'json', toml: 'toml',
  yaml: 'yaml', html: 'html', css: 'css', sql: 'sql', markdown: 'md',
};

/** The file name for `language` (`snippet.rs` for "Rust"). */
function fileName(language: string | undefined): string {
  return `snippet.${EXTENSIONS[(language ?? '').trim().toLowerCase()] ?? 'txt'}`;
}

export default definePlugin({
  activate() {
    commands.register(
      new SlashCommand('snippet', 'Send code as a file').text('code', 'The code', true).text('language', 'Its language (rust, python, …)'),
      (inv) => {
        const code = commands.text(inv, 'code') ?? '';
        if (!code.trim()) return commands.local('Nothing to send.');
        const name = fileName(commands.text(inv, 'language'));
        try {
          sendMessage(inv.channel.id, '', [file(name, 'text/plain', new TextEncoder().encode(code))]);
        } catch (e) {
          return commands.local(`Couldn't send it: ${e instanceof RekindleError ? e.code : e}`);
        }
        return commands.none();
      },
    );
  },
});
