import { set } from 'rekindle:plugin/theming@0.8.0';
export function activate() {
  set('accent', { tag: 'color', val: { r: 0, g: 0, b: 0, a: 255 } });
}
