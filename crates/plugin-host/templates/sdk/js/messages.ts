// rekindle/messages: message content your client has received (needs
// `read-messages`).
//
// Only what the client has loaded: scrolling up loads more, plugins can't.
// The one exception is `resolveLink`, which makes the fetch the client makes
// when the user clicks a message link that's on screen.

import * as raw from 'rekindle:plugin/messages@0.8.0';
import type { MessageRef } from 'rekindle:plugin/types@0.8.0';
import type { Message } from 'rekindle:plugin/records@0.8.0';
import { call, optU64, u64, type Id } from 'rekindle';

export type { Attachment, Embed, EmbedMedia, Forwarded, Message, Poll, Reaction } from 'rekindle:plugin/records@0.8.0';
export type LinkError = raw.LinkError;

export function get(channel: Id, id: Id): Message | undefined {
  return raw.getMessage(u64(channel), u64(id));
}

/** Loaded messages, newest first, older than `before` if given (at most 100). */
export function loaded(channel: Id, before?: Id, limit = 50): Message[] {
  return raw.loadedMessages(u64(channel), optU64(before), limit);
}

/** Messages on screen, top to bottom. */
export function visible(): MessageRef[] {
  return raw.visibleMessages();
}

export function pins(channel: Id): MessageRef[] {
  return raw.pins(u64(channel));
}

/**
 * The message a link (`https://discord.com/channels/g/c/m`) points to, for a
 * link preview. Works only while a message on screen contains the link;
 * rate-limited. Throws a RekindleError ('not-visible', 'invalid',
 * 'not-found', 'rate-limited', 'unavailable').
 */
export function resolveLink(url: string): Message {
  return call('messages.resolveLink', () => raw.resolveMessageLink(url));
}

const LINK_PREFIXES = ['https://discord.com/channels/', 'https://ptb.discord.com/channels/', 'https://canary.discord.com/channels/'];

/** The message links in a text, without the punctuation after their last digit. */
export function linksIn(text: string): string[] {
  return text
    .split(/[\s<>()]+/)
    .map((w) => w.replace(/[^0-9]+$/, ''))
    .filter((w) => LINK_PREFIXES.some((p) => w.startsWith(p)));
}

/** A message's reference (for actions and accessories). */
export function reference(m: Message): MessageRef {
  return { id: m.id, channel: m.channel, guild: m.guild, author: m.author.id };
}
