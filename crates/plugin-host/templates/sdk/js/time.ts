// rekindle/time: dates and times without a wall clock (no capability). The
// client formats in the user's locale and timezone, and you never learn
// either.
//
//   time.format(time.snowflakeMs(user.id), 'relative');   // "3 years ago"
//   time.duration(62_000);                                // "1:02"

import * as host from 'rekindle:plugin/host@0.10.0';
import { u64, type Id } from 'rekindle';

export type TimeStyle = host.TimeStyle;

/** When a snowflake (user, message, guild id, ...) was created, epoch ms. */
export function snowflakeMs(id: Id): number {
  return Number(host.snowflakeMs(u64(id)));
}

/** Format an epoch-ms time in one of Discord's timestamp styles (`'short-time'`, `'long-date'`, `'relative'`, ...). */
export function format(epochMs: Id, style: TimeStyle): string {
  return host.formatTime(u64(epochMs), style);
}

/** "0:42", "1:02:03", "3 days". */
export function duration(millis: Id): string {
  return host.formatDuration(u64(millis));
}
