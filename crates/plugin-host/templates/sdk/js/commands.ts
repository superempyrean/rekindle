// rekindle/commands: local slash commands (needs `ui`), listed next to the
// client's own and marked with your plugin's name. Options arrive typed.
//
//   commands.register(new SlashCommand('shrug', 'Append a shrug').text('message', 'What to say'), (inv) =>
//     commands.insert(`${commands.text(inv, 'message') ?? ''} ¯\\_(ツ)_/¯`),
//   );
//
// Replies: `local` (an "only you can see this" message), `insert` (into the
// composer), `send` (as your message; needs `compose`, and still passes every
// compose hook), or nothing.

import * as raw from 'rekindle:plugin/ui@0.9.0';
import type { CommandOption, CommandReply, Invocation, OptionKind, OptionValue } from 'rekindle:plugin/command-types@0.9.0';
import type { Span } from 'rekindle:plugin/types@0.9.0';
import { __registry, call, span } from 'rekindle';

export type { CommandReply, Invocation, OptionKind, OptionValue };

/** A slash command, built up. */
export class SlashCommand {
  readonly options: CommandOption[] = [];
  inGuilds = true;
  inDms = true;

  /** `name`: lower-case, 1-32 characters. */
  constructor(
    readonly name: string,
    readonly description: string,
  ) {}

  option(name: string, description: string, kind: OptionKind, required = false): this {
    this.options.push({ name, description, kind, required, choices: [], min: undefined, max: undefined, autocomplete: false });
    return this;
  }
  text(name: string, description: string, required = false): this {
    return this.option(name, description, 'text', required);
  }
  boolean(name: string, description: string, required = false): this {
    return this.option(name, description, 'boolean', required);
  }
  integer(name: string, description: string, required = false): this {
    return this.option(name, description, 'integer', required);
  }
  number(name: string, description: string, required = false): this {
    return this.option(name, description, 'number', required);
  }
  user(name: string, description: string, required = false): this {
    return this.option(name, description, 'user', required);
  }
  channel(name: string, description: string, required = false): this {
    return this.option(name, description, 'channel', required);
  }
  /** One of `choices`, as `[label, value]` pairs. */
  choice(name: string, description: string, choices: Array<[string, string]>, required = false): this {
    this.option(name, description, 'choice', required);
    this.options[this.options.length - 1].choices = choices;
    return this;
  }
  /** Ask for suggestions on the last option as the user types (`register`'s `complete`, or `onAutocomplete`). */
  autocomplete(): this {
    const last = this.options[this.options.length - 1];
    if (last) last.autocomplete = true;
    return this;
  }
  guildsOnly(): this {
    this.inDms = false;
    return this;
  }
  dmsOnly(): this {
    this.inGuilds = false;
    return this;
  }
}

/**
 * Register a slash command. `run`, if given, answers it (instead of
 * `onSlashCommand`); `complete`, if given, answers autocomplete for it
 * (instead of `onAutocomplete`).
 */
export function register(
  command: SlashCommand,
  run?: (inv: Invocation) => CommandReply | undefined | void,
  complete?: (option: string, partial: string) => Array<[string, string]> | undefined | void,
): void {
  const { name, description, options, inGuilds, inDms } = command;
  call('commands.register', () => raw.registerSlashCommand({ name, description, options, inGuilds, inDms }));
  if (run) __registry.slash.set(name, run);
  if (complete) __registry.autocomplete.set(name, complete);
}

export function unregister(name: string): void {
  raw.unregisterSlashCommand(name);
  __registry.slash.delete(name);
  __registry.autocomplete.delete(name);
}

function opt(inv: Invocation, name: string): OptionValue | undefined {
  return inv.options.find(([n]) => n === name)?.[1];
}

/** A text (or choice) option's value. */
export function text(inv: Invocation, name: string): string | undefined {
  const v = opt(inv, name);
  return v?.tag === 'text' || v?.tag === 'choice' ? v.val : undefined;
}

export function boolean(inv: Invocation, name: string): boolean | undefined {
  const v = opt(inv, name);
  return v?.tag === 'boolean' ? v.val : undefined;
}

/** An integer option (a bigint: options are 64-bit). */
export function integer(inv: Invocation, name: string): bigint | undefined {
  const v = opt(inv, name);
  return v?.tag === 'integer' ? v.val : undefined;
}

export function number(inv: Invocation, name: string): number | undefined {
  const v = opt(inv, name);
  return v?.tag === 'number' ? v.val : undefined;
}

/** A user, channel or role option's id. */
export function id(inv: Invocation, name: string): bigint | undefined {
  const v = opt(inv, name);
  return v?.tag === 'user' || v?.tag === 'channel' || v?.tag === 'role' ? v.val : undefined;
}

/** An "only you can see this" reply: plain text or spans. */
export function local(content: string | Span[]): CommandReply {
  return { tag: 'local', val: typeof content === 'string' ? [span(content)] : content };
}

/** Put `text` in the composer for the user to edit and send. */
export function insert(text: string): CommandReply {
  return { tag: 'insert', val: text };
}

/** Send `text` as the user's message (needs `compose`). */
export function send(text: string): CommandReply {
  return { tag: 'send', val: text };
}

export function none(): CommandReply {
  return { tag: 'none' };
}
