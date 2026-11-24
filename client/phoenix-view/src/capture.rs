//! A PNG of the window's own client area, for the self-test.
//!
//! `PrintWindow` with `PW_RENDERFULLCONTENT` asks DWM for what the window
//! shows, swap chain included, so it works while the window sits off the
//! desktop. It reads this process's own window and sends no input.

use std::path::Path;

use anyhow::{Result, bail};

#[repr(C)]
struct BitmapInfoHeader {
    size: u32,
    width: i32,
    height: i32,
    planes: u16,
    bit_count: u16,
    compression: u32,
    size_image: u32,
    x_ppm: i32,
    y_ppm: i32,
    clr_used: u32,
    clr_important: u32,
}

#[link(name = "user32")]
unsafe extern "system" {
    fn GetClientRect(hwnd: isize, rect: *mut [i32; 4]) -> i32;
    fn GetDC(hwnd: isize) -> isize;
    fn ReleaseDC(hwnd: isize, dc: isize) -> i32;
    fn PrintWindow(hwnd: isize, dc: isize, flags: u32) -> i32;
}

#[link(name = "gdi32")]
unsafe extern "system" {
    fn CreateCompatibleDC(dc: isize) -> isize;
    fn DeleteDC(dc: isize) -> i32;
    fn CreateDIBSection(
        dc: isize,
        info: *const BitmapInfoHeader,
        usage: u32,
        bits: *mut *mut u8,
        section: isize,
        offset: u32,
    ) -> isize;
    fn SelectObject(dc: isize, obj: isize) -> isize;
    fn DeleteObject(obj: isize) -> i32;
}

const PW_CLIENTONLY: u32 = 1;
const PW_RENDERFULLCONTENT: u32 = 2;

/// The client area as RGBA rows, top to bottom.
pub struct Shot {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Shot {
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.width + x) * 4) as usize;
        [
            self.rgba[i],
            self.rgba[i + 1],
            self.rgba[i + 2],
            self.rgba[i + 3],
        ]
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let file = std::io::BufWriter::new(std::fs::File::create(path)?);
        let mut enc = png::Encoder::new(file, self.width, self.height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()?.write_image_data(&self.rgba)?;
        Ok(())
    }
}

pub fn client_area(window: &gpui::Window) -> Result<Shot> {
    let Some(hwnd) = crate::hwnd(window) else {
        bail!("no window handle")
    };
    let mut r = [0i32; 4];
    // SAFETY: the window's own handle and a RECT-sized buffer.
    if unsafe { GetClientRect(hwnd, &raw mut r) } == 0 {
        bail!("GetClientRect failed");
    }
    let (w, h) = (r[2] - r[0], r[3] - r[1]);
    if w <= 0 || h <= 0 {
        bail!("an empty client area");
    }
    let info = BitmapInfoHeader {
        size: size_of::<BitmapInfoHeader>() as u32,
        width: w,
        // Negative: rows top to bottom.
        height: -h,
        planes: 1,
        bit_count: 32,
        compression: 0,
        size_image: 0,
        x_ppm: 0,
        y_ppm: 0,
        clr_used: 0,
        clr_important: 0,
    };
    // SAFETY: GDI calls on a memory DC and a DIB section this function
    // creates and frees; the bits pointer is read only while the section
    // lives, for the w * h * 4 bytes it was created with.
    unsafe {
        let screen = GetDC(hwnd);
        let dc = CreateCompatibleDC(screen);
        let mut bits: *mut u8 = std::ptr::null_mut();
        let bmp = CreateDIBSection(dc, &raw const info, 0, &raw mut bits, 0, 0);
        ReleaseDC(hwnd, screen);
        if bmp == 0 || bits.is_null() {
            DeleteDC(dc);
            bail!("CreateDIBSection failed");
        }
        let old = SelectObject(dc, bmp);
        let ok = PrintWindow(hwnd, dc, PW_CLIENTONLY | PW_RENDERFULLCONTENT);
        let bgra = std::slice::from_raw_parts(bits, (w * h * 4) as usize);
        let mut rgba = Vec::with_capacity(bgra.len());
        for p in bgra.as_chunks::<4>().0 {
            rgba.extend_from_slice(&[p[2], p[1], p[0], 255]);
        }
        SelectObject(dc, old);
        DeleteObject(bmp);
        DeleteDC(dc);
        if ok == 0 {
            bail!("PrintWindow failed");
        }
        Ok(Shot {
            width: w as u32,
            height: h as u32,
            rgba,
        })
    }
}
