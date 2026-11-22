// A compose hook without the `compose` capability: the client would never
// call it, so the build says so.
export function onCompose(draft) {
  return draft.text.toUpperCase();
}
