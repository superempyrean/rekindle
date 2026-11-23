// rekindle/render: change how messages look (needs `render-messages`).
//
// Export a `render(input)` handler: it runs in the **render isolate**, a
// second instance of your plugin that sees message content but can't keep or
// send it (no storage writes, no network, no timers, no UI calls), and never
// activates. Its only output is the transforms built here, which the client
// draws.
//
//   export default definePlugin({
//     render(input) {
//       const t = render.highlightAll(input.message.content, 'rekindle', 'text-brand');
//       return t ? [t] : [];
//     },
//   });
//
// Output rules the client enforces: no URLs (span targets can't be links or
// media), images are your registered assets, and accessory buttons may only
// use ids your main instance declared with `declareActions` (their clicks
// reach the main instance's `onUi` with surface `message:<id>`). Results are
// cached per message; call `invalidate` from the main instance when your
// rules change (settings changes invalidate by themselves).
//
// Highlight ranges are UTF-8 byte offsets into the content, as the contract
// counts them; `highlightAll` and `byteOffset` do the conversion.

import * as raw from 'rekindle:plugin/render-control@0.10.0';
import type { Badge, Highlight, RenderInput, Transform } from 'rekindle:plugin/render-types@0.10.0';
import type { Color, Icon, Span, SpanColor } from 'rekindle:plugin/types@0.10.0';
import { optU64, rgba, token, type Id } from 'rekindle';
import { toElements, type Element, type Elements } from 'rekindle/elements';

export type { Badge, Highlight, RenderInput, Transform };

/** A token name, a color, or a `SpanColor`. */
export type Paint = string | Color | SpanColor;

function paint(p: Paint | undefined): SpanColor | undefined {
  return p === undefined ? undefined : typeof p === 'string' ? token(p) : 'tag' in p ? p : rgba(p);
}

/** Accessory button ids the isolate may use (call from the main instance, e.g. in `activate`). */
export function declareActions(ids: string[]): void {
  raw.declareActions(ids);
}

/** Re-render everything, or one channel's messages (main instance only). */
export function invalidate(channel?: Id): void {
  raw.invalidate(optU64(channel));
}

/** The UTF-8 length of a string. */
export function utf8Length(s: string): number {
  let n = 0;
  for (let i = 0; i < s.length; i++) {
    const c = s.charCodeAt(i);
    if (c < 0x80) n += 1;
    else if (c < 0x800) n += 2;
    else if (c >= 0xd800 && c <= 0xdbff && i + 1 < s.length) {
      n += 4;
      i++;
    } else n += 3;
  }
  return n;
}

/** The UTF-8 byte offset of string index `index` in `s` (what highlight ranges count). */
export function byteOffset(s: string, index: number): number {
  return utf8Length(s.slice(0, index));
}

/** Show a "hidden by <plugin>: reason" bar with a Show button instead. */
export function collapse(reason: string): Transform {
  return { tag: 'collapse', val: reason };
}

/** Blur the content until clicked, like a spoiler. */
export function blur(): Transform {
  return { tag: 'blur' };
}

export interface HighlightStyle {
  color?: Paint;
  bold?: boolean;
  tooltip?: string;
}

/** Style a UTF-8 byte range of the content. */
export function highlight(start: number, end: number, style: HighlightStyle = {}): Transform {
  return { tag: 'highlight', val: [{ start, end, color: paint(style.color), bold: style.bold ?? false, tooltip: style.tooltip }] };
}

/**
 * Highlight every case-insensitive occurrence of `needle` in `content`, or
 * `undefined` if there's none.
 */
export function highlightAll(content: string, needle: string, style: HighlightStyle | Paint = {}): Transform | undefined {
  if (!needle) return undefined;
  const s: HighlightStyle = typeof style === 'string' || 'tag' in style || 'r' in style ? { color: style as Paint } : style;
  const hay = content.toLowerCase();
  const pin = needle.toLowerCase();
  // Lower-casing can change lengths; only use it when it didn't.
  if (hay.length !== content.length) return undefined;
  const ranges: Highlight[] = [];
  // `from` is a string index into `content`; `bytes` is its UTF-8 offset.
  let from = 0;
  let bytes = 0;
  for (let i = hay.indexOf(pin, from); i >= 0; i = hay.indexOf(pin, from)) {
    bytes += utf8Length(content.slice(from, i));
    const len = utf8Length(content.slice(i, i + pin.length));
    ranges.push({ start: bytes, end: bytes + len, color: paint(s.color), bold: s.bold ?? false, tooltip: s.tooltip });
    bytes += len;
    from = i + pin.length;
  }
  return ranges.length ? { tag: 'highlight', val: ranges } : undefined;
}

/** Inline spans after the content ("(translated)", "edited 3x"). */
export function append(spans: Span[]): Transform {
  return { tag: 'append', val: spans };
}

/** Replace how the content reads (the original stays one click away). */
export function replaceContent(spans: Span[]): Transform {
  return { tag: 'replace-content', val: spans };
}

/** Re-highlight the nth code block. */
export function codeBlock(n: number, spans: Span[]): Transform {
  return { tag: 'code-block', val: [n, spans] };
}

/** Tint the message row. */
export function background(color: Paint): Transform {
  return { tag: 'background', val: paint(color) as SpanColor };
}

/** A badge after the author's name. */
export function authorBadge(badge: { tooltip: string; icon?: Icon; image?: string; text?: string; color?: Color }): Transform {
  return { tag: 'author-badge', val: { icon: badge.icon, image: badge.image, text: badge.text, tooltip: badge.tooltip, color: badge.color } };
}

/** Override the author name's color for this message. */
export function authorColor(color: Color): Transform {
  return { tag: 'author-color', val: color };
}

/** Elements under the message. Buttons need ids declared with `declareActions`. */
export function accessory(elements: Elements | Element[]): Transform {
  return { tag: 'accessory', val: toElements(elements) };
}

/** Hide the nth embed. */
export function hideEmbed(n: number): Transform {
  return { tag: 'hide-embed', val: n };
}

/** Hide the nth attachment. */
export function hideAttachment(n: number): Transform {
  return { tag: 'hide-attachment', val: n };
}
