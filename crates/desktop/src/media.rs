//! The client's image fetch, found by its `tracing` span.
//!
//! Images the client draws from a URL, such as a profile's badge icons, are
//! fetched by `libdiscord_media::fetch(this, url: &str, max_bytes, a, b) ->
//! Pin<Box<dyn Future>>`, which builds a request of its own outside the
//! REST transport. The URL is in `rdx` and `r8`, and the future borrows it
//! until it is done. `a` and `b` are on the stack; the client's one call
//! passes `8` and `0` on build 2208. The box comes back in `rax` and its
//! vtable in `rdx`.
//!
//! The function opens a `tracing` span named `media_fetch` with the target
//! `libdiscord_media::fetch`. The two names sit next to each other in the
//! image, and the span's `Metadata` static holds each as a `&str`, the name
//! first, then the target. The span's callsite is the static the metadata
//! points at past them, and `fetch` is the one function that loads it.

use anyhow::{Context, Result};

use crate::image::{Image, one_of};

const NAMES: &[u8] = b"media_fetchlibdiscord_media::fetch";
const NAME_LEN: u64 = 11;
const TARGET_LEN: u64 = 23;

/// The span's callsite statics: the data pointers in its `Metadata` past
/// the two names that no function but one loads.
fn metadata(image: &Image<'_>) -> Result<Vec<u64>> {
    let name = one_of("the media fetch's span names", &image.find(NAMES))?;
    let target = name + NAME_LEN;
    let mut out = Vec::new();
    for (start, end) in image.data() {
        let mut at = start;
        while at + 0x40 <= end {
            if image.qword(at) == Some(name)
                && image.qword(at + 8) == Some(NAME_LEN)
                && image.qword(at + 16) == Some(target)
                && image.qword(at + 24) == Some(TARGET_LEN)
            {
                out.push(at);
            }
            at += 8;
        }
    }
    Ok(out)
}

/// `libdiscord_media::fetch`.
pub fn fetch(image: &Image<'_>) -> Result<u64> {
    let meta = one_of("the media fetch's span metadata", &metadata(image)?)?;
    // The words after the names that point into the image's data, in order:
    // the module path, the file and the callsite. Of those, the callsite is
    // the one a single function loads.
    let data = image.data();
    let pointers: Vec<u64> = (4..16)
        .filter_map(|k| image.qword(meta + 8 * k))
        .filter(|p| data.iter().any(|&(s, e)| (s..e).contains(p)))
        .collect();
    let mut found = Vec::new();
    for p in pointers {
        let mut funcs: Vec<u64> = image
            .leas_into(&[p])
            .into_iter()
            .filter_map(|(lea, _)| image.function(lea).map(|f| f.0))
            .collect();
        funcs.sort_unstable();
        funcs.dedup();
        if let [one] = funcs.as_slice() {
            found.push(*one);
        }
    }
    found.sort_unstable();
    found.dedup();
    one_of("libdiscord_media::fetch", &found).context("the function that loads the span's callsite")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_fetch_on_build_2208() {
        let Ok(path) = std::env::var("REKINDLE_DESKTOP_EXE") else {
            eprintln!("REKINDLE_DESKTOP_EXE not set; skipped");
            return;
        };
        let bytes = std::fs::read(path).unwrap();
        let image = Image::parse(&bytes).unwrap();
        let f = fetch(&image).unwrap();
        assert_eq!(f - image.base(), 0x7991620);
    }
}
