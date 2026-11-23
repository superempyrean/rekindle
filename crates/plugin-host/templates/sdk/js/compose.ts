// rekindle/compose: results for `onCompose` and `onUpload` (the hooks need
// `compose`; this module itself imports nothing).
//
//   onCompose(draft) {
//     if (!draft.text.includes(':shrug:')) return compose.keep();
//     return compose.replace(draft.text.replaceAll(':shrug:', '¯\\_(ツ)_/¯'));
//   }
//
// You see the draft after earlier plugins' rewrites. The client refuses a
// result that's blank, over 4000 characters or has control characters, and
// tells the user which plugins changed their message. Returning a string
// from `onCompose` is short for `replace(text)`, nothing for `keep()`.

import type {
  AttachmentInfo,
  ComposeKind,
  ComposeResult,
  Draft,
  ReplyInfo,
  Rewrite,
  Upload,
  UploadResult,
} from 'rekindle:plugin/compose-types@0.10.0';

export type { AttachmentInfo, ComposeKind, ComposeResult, Draft, ReplyInfo, Rewrite, Upload, UploadResult };

/** Leave the message as it is. */
export function keep(): ComposeResult {
  return { tag: 'keep' };
}

/** Send `text` instead; optionally change whether a reply pings, or send it silently. */
export function replace(text: string, opts: { mention?: boolean; silent?: boolean } = {}): ComposeResult {
  return { tag: 'replace', val: { text, mention: opts.mention, silent: opts.silent } };
}

/** Keep the text; change whether the reply pings its author (NoReplyMention). */
export function setMention(draft: Draft, mention: boolean): ComposeResult {
  return replace(draft.text, { mention });
}

/** Don't send: keep the draft in the composer and tell the user why. */
export function cancel(reason: string): ComposeResult {
  return { tag: 'cancel', val: reason };
}

/** Upload the file as it is. */
export function keepUpload(): UploadResult {
  return { tag: 'keep' };
}

/** Upload this instead (stripped EXIF, a new name, another format). */
export function replaceUpload(upload: Upload): UploadResult {
  return { tag: 'replace', val: upload };
}

/** Remove the attachment, telling the user why. */
export function dropUpload(reason: string): UploadResult {
  return { tag: 'drop', val: reason };
}
