// rekindle/overlay: sprites on a transparent layer over the client (needs
// `overlay`).
//
//   overlay.registerImage('cat', pngBytes);
//   overlay.draw({ instance: 'cat-1', image: 'cat', src: [0, 0, 32, 32], dst: [x, y, 32, 32] });

import * as raw from 'rekindle:plugin/overlay@0.11.0';
import { call } from 'rekindle';

export type Sprite = raw.Sprite;

/** Register a PNG to draw sprites from. */
export function registerImage(id: string, png: Uint8Array): void {
  call('overlay.registerImage', () => raw.registerImage(id, png));
}

export function removeImage(id: string): void {
  raw.removeImage(id);
}

/**
 * Draw (or move) a sprite: the `src` rectangle `[x, y, w, h]` of `image`, at
 * `dst` `[x, y, w, h]` in client pixels. Re-drawing an `instance` moves it.
 */
export function draw(s: { instance: string; image: string; src: [number, number, number, number]; dst: [number, number, number, number] }): void {
  const [sx, sy, sw, sh] = s.src;
  const [x, y, w, h] = s.dst;
  call('overlay.draw', () => raw.draw({ instance: s.instance, image: s.image, sx, sy, sw, sh, x, y, w, h }));
}

/** Remove a sprite instance. */
export function clear(instance: string): void {
  raw.clear(instance);
}

/** The overlay's size in client pixels: `[width, height]`. */
export function viewport(): [number, number] {
  return raw.viewport();
}
