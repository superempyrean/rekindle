// rekindle/elements: element lists for panels, modals, the settings page's
// extras, profile sections, message accessories and render accessories (no
// capability: building a list needs nothing; showing it does).
//
//   const els = new Elements().heading('Stats').field('Messages', '42').button('reset', 'Reset');
//   decorations.setAccessory(message, els);
//
// `Panel` (`rekindle/ui`) is an `Elements` with an id and a title. Read the
// value an `onUi` event carries with `uiValue`.

import type { Color, Element, Icon, Span, UiEvent, UiValue } from 'rekindle:plugin/types@0.10.0';
import { __registry, u64, type Id } from 'rekindle';

export type { Element };

/** Element constructors (each returns one contract `Element`). */
export const element = {
  heading: (text: string): Element => ({ tag: 'heading', val: text }),
  text: (text: string): Element => ({ tag: 'text', val: text }),
  /** Styled text: spans from `span(...)`. */
  richText: (spans: Span[]): Element => ({ tag: 'rich-text', val: spans }),
  separator: (): Element => ({ tag: 'separator' }),
  toggle: (id: string, label: string, value: boolean): Element => ({ tag: 'toggle', val: { id, label, value } }),
  slider: (id: string, label: string, value: number, { min, max, step = 1 }: { min: number; max: number; step?: number }): Element => ({
    tag: 'slider',
    val: { id, label, min, max, step, value },
  }),
  colorPicker: (id: string, label: string, value: Color): Element => ({ tag: 'color-picker', val: { id, label, value } }),
  textInput: (id: string, label: string, value = '', placeholder = '', multiline = false): Element => ({
    tag: 'text-input',
    val: { id, label, value, placeholder, multiline },
  }),
  button: (id: string, label: string, opts: { icon?: Icon; danger?: boolean } = {}): Element => ({
    tag: 'button',
    val: { id, label, icon: opts.icon, danger: opts.danger ?? false },
  }),
  select: (id: string, label: string, options: string[], selected = 0): Element => ({
    tag: 'select',
    val: { id, label, options, selected },
  }),
  /** Rows of cells, one per column; edits arrive as `rows`. */
  table: (id: string, label: string, columns: string[], rows: string[][]): Element => ({ tag: 'table', val: { id, label, columns, rows } }),
  /** An editable list of strings; edits arrive as `items`. */
  stringList: (id: string, label: string, items: string[], placeholder = ''): Element => ({
    tag: 'string-list',
    val: { id, label, items, placeholder },
  }),
  /** One of your registered images (`ui.registerImage`). */
  image: (id: string, width: number, height: number): Element => ({ tag: 'image', val: [id, width, height] }),
  /** A user, drawn the client's way (avatar and name). */
  user: (id: Id): Element => ({ tag: 'user', val: u64(id) }),
  /** A key/value row ("Created", "3 years ago"). */
  field: (key: string, value: string): Element => ({ tag: 'field', val: [key, value] }),
};

/** A chainable element list. */
export class Elements {
  readonly elements: Element[] = [];

  /** Add any elements (see `element` for constructors). */
  add(...elements: Element[]): this {
    this.elements.push(...elements);
    return this;
  }
  heading(text: string): this {
    return this.add(element.heading(text));
  }
  text(text: string): this {
    return this.add(element.text(text));
  }
  richText(spans: Span[]): this {
    return this.add(element.richText(spans));
  }
  separator(): this {
    return this.add(element.separator());
  }
  toggle(id: string, label: string, value: boolean): this {
    return this.add(element.toggle(id, label, value));
  }
  slider(id: string, label: string, value: number, opts: { min: number; max: number; step?: number }): this {
    return this.add(element.slider(id, label, value, opts));
  }
  colorPicker(id: string, label: string, value: Color): this {
    return this.add(element.colorPicker(id, label, value));
  }
  textInput(id: string, label: string, value = '', placeholder = ''): this {
    return this.add(element.textInput(id, label, value, placeholder));
  }
  textArea(id: string, label: string, value = '', placeholder = ''): this {
    return this.add(element.textInput(id, label, value, placeholder, true));
  }
  button(id: string, label: string): this {
    return this.add(element.button(id, label));
  }
  iconButton(id: string, icon: Icon, label: string): this {
    return this.add(element.button(id, label, { icon }));
  }
  dangerButton(id: string, label: string): this {
    return this.add(element.button(id, label, { danger: true }));
  }
  /** A dropdown; `selected` is the index into `options`. */
  select(id: string, label: string, options: string[], selected = 0): this {
    return this.add(element.select(id, label, options, selected));
  }
  table(id: string, label: string, columns: string[], rows: string[][]): this {
    return this.add(element.table(id, label, columns, rows));
  }
  stringList(id: string, label: string, items: string[], placeholder = ''): this {
    return this.add(element.stringList(id, label, items, placeholder));
  }
  image(id: string, width: number, height: number): this {
    return this.add(element.image(id, width, height));
  }
  user(id: Id): this {
    return this.add(element.user(id));
  }
  field(key: string, value: string): this {
    return this.add(element.field(key, value));
  }
}

/** An element list, as the contract takes it. */
export function toElements(e: Elements | Element[]): Element[] {
  return Array.isArray(e) ? e : e.elements;
}

/**
 * Read an `onUi` value by kind: `uiValue.text(ev.value)` is the text if it's
 * a text change, else `undefined`. `clicked` is `true` for a button click.
 */
export const uiValue = {
  toggled: (v: UiValue): boolean | undefined => (v.tag === 'toggled' ? v.val : undefined),
  number: (v: UiValue): number | undefined => (v.tag === 'number' ? v.val : undefined),
  color: (v: UiValue): Color | undefined => (v.tag === 'color' ? v.val : undefined),
  text: (v: UiValue): string | undefined => (v.tag === 'text' ? v.val : undefined),
  selected: (v: UiValue): number | undefined => (v.tag === 'selected' ? v.val : undefined),
  rows: (v: UiValue): string[][] | undefined => (v.tag === 'rows' ? v.val : undefined),
  items: (v: UiValue): string[] | undefined => (v.tag === 'items' ? v.val : undefined),
  clicked: (v: UiValue): boolean => v.tag === 'clicked',
};

/**
 * Call `handler` when the user changes `element` on `surface` (a panel or
 * modal id, `settings-page`, `notice`, or `message:<id>`), with the new value
 * (a toggle gives a boolean, a text input a string, ...; a button gives
 * `undefined`). Runs before `onUi`. Replaces an earlier callback for it.
 */
export function on(surface: string, element: string, handler: (value: any, event: UiEvent) => void): void {
  __registry.ui.set(`${surface}\u0000${element}`, (value, event) => handler(value, event));
}
