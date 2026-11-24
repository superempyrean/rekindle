//! Remote images: an image node whose `uri` is an `https` URL, such as a
//! server icon or an avatar on `cdn.discordapp.com`.
//!
//! The Android platform loads these itself (`dd` in
//! `com.discord.nativeapp.modules`, build 356207): it fetches the URL,
//! decodes a bitmap, draws it with the node's `scale`, `radius` and `tint`,
//! and raises `load` (`width`, `height` and `uri`) or `error` (`uri`) on
//! the node when its `on` lists them. The guest asks for nothing else: the
//! `ImageLoads` host module and the `ImageLoadStatus` and `ImageEventUri`
//! capabilities stay off unless `DeviceInfo.constants` lists them.
//!
//! Here a few worker threads fetch through [`Net::get_image`], which lets
//! only Discord's hosts through, and decode PNG, JPEG, WebP and GIF. An
//! animated GIF or WebP shows its first frame.

use std::io::Cursor;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use futures::channel::mpsc::UnboundedSender;
use gpui::RenderImage;
use image::{AnimationDecoder as _, Frame, ImageFormat};

use crate::net::Net;

const WORKERS: usize = 4;

/// A decoded image and its size in pixels.
pub struct Decoded {
    pub image: Arc<RenderImage>,
    pub width: u32,
    pub height: u32,
    pub how: String,
}

/// A finished load: the key it was asked for under, and the image or why
/// there is none.
pub struct Done {
    pub key: String,
    pub result: Result<Decoded, String>,
}

struct Job {
    key: String,
    url: String,
    tint: Option<u32>,
}

/// The worker threads' queue.
pub struct Loader {
    jobs: mpsc::Sender<Job>,
}

impl Loader {
    pub fn new(net: &Arc<Net>, done: &UnboundedSender<Done>) -> Loader {
        let (tx, rx) = mpsc::channel::<Job>();
        let rx = Arc::new(Mutex::new(rx));
        for n in 0..WORKERS {
            let (rx, net, done) = (rx.clone(), net.clone(), done.clone());
            let _ = std::thread::Builder::new()
                .name(format!("image-{n}"))
                .spawn(move || {
                    loop {
                        let job = rx.lock().unwrap().recv();
                        let Ok(job) = job else { return };
                        let result = net
                            .get_image(&job.url)
                            .and_then(|bytes| decode(&bytes, job.tint));
                        if done
                            .unbounded_send(Done {
                                key: job.key,
                                result,
                            })
                            .is_err()
                        {
                            return;
                        }
                    }
                });
        }
        Loader { jobs: tx }
    }

    /// Fetch and decode `url`, tinted with `tint`; the result arrives as a
    /// [`Done`] under `key`.
    pub fn load(&self, key: String, url: String, tint: Option<u32>) {
        let _ = self.jobs.send(Job { key, url, tint });
    }
}

/// The format the bytes start with, whatever the URL's extension says.
pub fn sniff(bytes: &[u8]) -> Option<ImageFormat> {
    match bytes {
        [0x89, b'P', b'N', b'G', ..] => Some(ImageFormat::Png),
        [0xff, 0xd8, 0xff, ..] => Some(ImageFormat::Jpeg),
        [b'G', b'I', b'F', b'8', ..] => Some(ImageFormat::Gif),
        [b'R', b'I', b'F', b'F', _, _, _, _, rest @ ..] if rest.starts_with(b"WEBP") => {
            Some(ImageFormat::WebP)
        }
        _ => None,
    }
}

/// Decode the first frame, tint it as Android's `SRC_IN` colour filter
/// does, and convert it to the BGRA GPUI draws.
pub fn decode(bytes: &[u8], tint: Option<u32>) -> Result<Decoded, String> {
    let format = sniff(bytes).ok_or_else(|| {
        format!(
            "not a PNG, JPEG, WebP or GIF ({} bytes starting {:02x?})",
            bytes.len(),
            &bytes[..bytes.len().min(4)]
        )
    })?;
    let mut rgba = if format == ImageFormat::Gif {
        // The first frame, composited as the decoder's frames are.
        image::codecs::gif::GifDecoder::new(Cursor::new(bytes))
            .map_err(|e| format!("GIF: {e}"))?
            .into_frames()
            .next()
            .ok_or("GIF: no frames")?
            .map_err(|e| format!("GIF: {e}"))?
            .into_buffer()
    } else {
        image::load_from_memory_with_format(bytes, format)
            .map_err(|e| format!("{format:?}: {e}"))?
            .into_rgba8()
    };
    let (width, height) = rgba.dimensions();
    if width == 0 || height == 0 {
        return Err(format!("{format:?}: empty"));
    }
    for px in rgba.pixels_mut() {
        let [r, g, b, a] = px.0;
        px.0 = match tint {
            Some(c) => {
                let ta = (c >> 24) as u8;
                [
                    (c & 0xff) as u8,
                    (c >> 8) as u8,
                    (c >> 16) as u8,
                    ((u16::from(a) * u16::from(ta)) / 255) as u8,
                ]
            }
            None => [b, g, r, a],
        };
    }
    Ok(Decoded {
        image: Arc::new(RenderImage::new(vec![Frame::new(rgba)])),
        width,
        height,
        how: format!(
            "{format:?} {width}x{height}{}",
            if tint.is_some() { ", tinted" } else { "" }
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32, px: [u8; 4]) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba(px));
        let mut out = Cursor::new(Vec::new());
        img.write_to(&mut out, ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn decodes_to_bgra() {
        let d = decode(&png(3, 2, [10, 20, 30, 255]), None).unwrap();
        assert_eq!((d.width, d.height), (3, 2));
        assert_eq!(&d.image.as_bytes(0).unwrap()[..4], &[30, 20, 10, 255]);
    }

    #[test]
    fn tint_keeps_alpha() {
        let d = decode(&png(1, 1, [10, 20, 30, 128]), Some(0xff_11_22_33)).unwrap();
        assert_eq!(d.image.as_bytes(0).unwrap(), &[0x33, 0x22, 0x11, 128]);
    }

    #[test]
    fn sniffs_by_content() {
        assert_eq!(sniff(b"RIFF\0\0\0\0WEBPVP8 "), Some(ImageFormat::WebP));
        assert_eq!(sniff(b"GIF89a"), Some(ImageFormat::Gif));
        assert_eq!(sniff(&[0xff, 0xd8, 0xff, 0xe0]), Some(ImageFormat::Jpeg));
        assert!(decode(b"<html>", None).is_err());
    }
}
