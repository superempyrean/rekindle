// Rules and their storage format (shared with the Rust plugin: fields joined
// by U+001F, records by U+001E).

export const MAX_RULES = 32;
const FS = '\u001f';
const RS = '\u001e';

export interface Rule {
  find: string;
  replace: string;
  regex: boolean;
}

/** A rule plus, for a regex rule, its compiled pattern or compile error. */
export interface Compiled {
  rule: Rule;
  error?: string;
  apply(text: string): string;
}

/**
 * A replacement's capture references, as the Rust plugin reads them (the
 * regex crate's syntax): `$1`, `${1}`, `$name`, `${name}`, and `$$` for a `$`.
 * A group that didn't take part, or doesn't exist, expands to nothing.
 */
export function expand(replacement: string, groups: (string | undefined)[], named?: Record<string, string | undefined>): string {
  return replacement.replace(/\$(\$|\{([^}]*)\}|([_0-9A-Za-z]+))/g, (_m, all: string, braced?: string, bare?: string) => {
    if (all === '$') return '$';
    const name = braced ?? bare ?? '';
    return (/^\d+$/.test(name) ? groups[Number(name)] : named?.[name]) ?? '';
  });
}

export function compile(rule: Rule): Compiled {
  if (!rule.find) return { rule, apply: (t) => t };
  if (!rule.regex) return { rule, apply: (t) => t.split(rule.find).join(rule.replace) };
  try {
    const re = new RegExp(rule.find, 'g');
    return {
      rule,
      // The replacer gets (match, ...groups, offset, input[, named groups]).
      apply: (t) =>
        t.replace(re, (...args: unknown[]) => {
          const named = typeof args[args.length - 1] === 'object' ? (args[args.length - 1] as Record<string, string>) : undefined;
          const groups = args.slice(0, named ? -3 : -2) as (string | undefined)[];
          return expand(rule.replace, groups, named);
        }),
    };
  } catch (e) {
    return { rule, error: e instanceof Error ? e.message : String(e), apply: (t) => t };
  }
}

export function decode(s: string): Rule[] {
  return s
    .split(RS)
    .filter((r) => r)
    .map((r) => r.split(FS))
    .filter((f) => f.length >= 3)
    .map(([find, replace, regex]) => ({ find, replace, regex: regex === '1' }))
    .slice(0, MAX_RULES);
}

export function encode(rules: Rule[]): string {
  return rules.map((r) => [r.find, r.replace, r.regex ? '1' : '0'].join(FS)).join(RS);
}
