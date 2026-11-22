// `fetch` is a Node/web global jco backs with a host interface
// (`jco:node/http`); rekindle plugins use the `http` capability instead.
export function activate() {
  fetch('https://example.com');
}
