// rekindle/audio: sounds (needs `audio`). Ship your own (Ogg/Opus or WAV, at
// most 1 MiB and 30 s, 16 per plugin), play them, replace the client's
// (`'message1'`, `'deafen'`, ...), and read text aloud with the client's
// local text-to-speech. Nothing is ever recorded.

import * as raw from 'rekindle:plugin/audio@0.11.0';
import { call } from 'rekindle';

export function register(id: string, data: Uint8Array): void {
  call('audio.register', () => raw.registerSound(id, data));
}

/** Play at `volume` (0..1, scaled by the client's notification volume). */
export function play(id: string, volume = 1): void {
  call('audio.play', () => raw.play(id, volume));
}

export function stop(id: string): void {
  raw.stop(id);
}

/** The client's own sound names. */
export function clientSounds(): string[] {
  return raw.clientSounds();
}

/** Replace a client sound with one of yours (`undefined` restores it). */
export function overrideSound(clientSound: string, sound: string | undefined): void {
  call('audio.overrideSound', () => raw.overrideSound(clientSound, sound));
}

/** Read `text` aloud, locally. Rate-limited. */
export function speak(text: string): void {
  call('audio.speak', () => raw.speak(text));
}
