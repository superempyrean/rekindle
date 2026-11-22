// rekindle/http: HTTPS to the hosts your manifest declares (needs `network`).
//
//   const res = http.get('https://api.example.com/palette');
//   if (res.ok) theme.set('accent', hex(res.json().accent));
//
// Requests are synchronous. The client allows only the exact hosts the user
// approved, HTTPS only, no redirects, and never attaches credentials. A
// refused or failed request throws a RekindleError ('host-not-allowed',
// 'insecure-scheme', 'invalid-url', 'timeout', 'too-large', 'transport').

import * as raw from 'rekindle:plugin/http@0.8.0';
import { call } from 'rekindle';

export type Method = raw.Method;

export class Response {
  constructor(private readonly r: raw.Response) {}

  get status(): number {
    return this.r.status;
  }
  /** 2xx. */
  get ok(): boolean {
    return this.r.status >= 200 && this.r.status < 300;
  }
  /** A header's value (case-insensitive name). */
  header(name: string): string | undefined {
    const n = name.toLowerCase();
    return this.r.headers.find(([k]) => k.toLowerCase() === n)?.[1] ?? undefined;
  }
  get headers(): [string, string][] {
    return this.r.headers;
  }
  bytes(): Uint8Array {
    return this.r.body;
  }
  text(): string {
    return new TextDecoder().decode(this.r.body);
  }
  json<T = unknown>(): T {
    return JSON.parse(this.text()) as T;
  }
}

export interface RequestInit {
  method?: Method;
  headers?: Record<string, string> | [string, string][];
  /** A string is sent as UTF-8; an object as JSON (with a JSON content-type). */
  body?: string | Uint8Array | object;
}

export function request(url: string, init: RequestInit = {}): Response {
  const headers: [string, string][] = Array.isArray(init.headers) ? [...init.headers] : Object.entries(init.headers ?? {});
  let body: Uint8Array | undefined;
  if (typeof init.body === 'string') body = new TextEncoder().encode(init.body);
  else if (init.body instanceof Uint8Array) body = init.body;
  else if (init.body !== undefined) {
    body = new TextEncoder().encode(JSON.stringify(init.body));
    if (!headers.some(([k]) => k.toLowerCase() === 'content-type')) headers.push(['content-type', 'application/json']);
  }
  const method = init.method ?? (body ? 'post' : 'get');
  return new Response(call(`http.${method}`, () => raw.fetch({ method, url, headers, body })));
}

export function get(url: string, init: Omit<RequestInit, 'method' | 'body'> = {}): Response {
  return request(url, { ...init, method: 'get' });
}

export function post(url: string, body: RequestInit['body'], init: Omit<RequestInit, 'method' | 'body'> = {}): Response {
  return request(url, { ...init, method: 'post', body });
}
