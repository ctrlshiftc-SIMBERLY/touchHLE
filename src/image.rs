/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Image decoding.
//!
//! Currently, supports PNG (treated as 8-bit sRGB), JPEG, BMP and GIF files.
//!
//! Implemented as a wrapper around the C library stb_image, since it supports
//! "CgBI" PNG files (an Apple proprietary extension used in iPhone OS apps).
//!
//! This module also exposes decompression for Imagination Technologies' PVRTC
//! format, implementing as a wrapper around their decoder from the PowerVR
//! SDK.
//!
//! References:
//! - "Supported Image Formats" in [Loading Images](https://developer.apple.com/library/archive/documentation/2DDrawing/Conceptual/DrawingPrintingiOS/LoadingImages/LoadingImages.html)

use std::ffi::{c_int, c_uchar, CStr};

use touchHLE_pvrt_decompress_wrapper::*;
use touchHLE_stb_image_wrapper::*;

pub struct Image {
    pixels: PixelStore,
    dimensions: (u32, u32),
}

enum PixelStore {
    StbImage(*mut c_uchar),
    Vec(Vec<u8>),
}

const PNG_MAGIC_NUMBER: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];

impl Image {
    /// Minimal decoder for uncompressed 8-bit RGB(A)/grey TIFFs (as used for
    /// some Particle Designer textures). stb_image doesn't support TIFF.
    fn from_tiff_bytes(bytes: &[u8]) -> Result<Image, String> {
        let be = bytes[0] == b'M';
        let rd16 = |o: usize| -> Result<u32, String> {
            let b = bytes.get(o..o + 2).ok_or("truncated TIFF")?;
            Ok(if be { u16::from_be_bytes([b[0], b[1]]) } else { u16::from_le_bytes([b[0], b[1]]) } as u32)
        };
        let rd32 = |o: usize| -> Result<u32, String> {
            let b = bytes.get(o..o + 4).ok_or("truncated TIFF")?;
            let a = [b[0], b[1], b[2], b[3]];
            Ok(if be { u32::from_be_bytes(a) } else { u32::from_le_bytes(a) })
        };
        let ifd = rd32(4)? as usize;
        let n = rd16(ifd)? as usize;
        let (mut width, mut height, mut spp, mut compression) = (0u32, 0u32, 1u32, 1u32);
        let mut bps = 8u32;
        let mut extra_associated = false;
        let mut offsets: Vec<u32> = Vec::new();
        let mut counts: Vec<u32> = Vec::new();
        for i in 0..n {
            let e = ifd + 2 + i * 12;
            let tag = rd16(e)?;
            let ty = rd16(e + 2)?;
            let count = rd32(e + 4)? as usize;
            let size = match ty { 3 => 2, 4 => 4, _ => 1 };
            let base = if size * count <= 4 { e + 8 } else { rd32(e + 8)? as usize };
            let val = |k: usize| -> Result<u32, String> {
                if ty == 3 { rd16(base + k * 2) } else if ty == 4 { rd32(base + k * 4) } else {
                    Ok(*bytes.get(base + k).ok_or("truncated TIFF")? as u32)
                }
            };
            match tag {
                256 => width = val(0)?,
                257 => height = val(0)?,
                258 => bps = val(0)?,
                259 => compression = val(0)?,
                277 => spp = val(0)?,
                273 => offsets = (0..count).map(val).collect::<Result<_, _>>()?,
                279 => counts = (0..count).map(val).collect::<Result<_, _>>()?,
                338 => extra_associated = val(0)? == 1,
                _ => (),
            }
        }
        if compression != 1 || bps != 8 || !(1..=4).contains(&spp) {
            return Err(format!("unsupported TIFF (compression {}, {} bits, {} samples)", compression, bps, spp));
        }
        let mut data = Vec::new();
        for (o, c) in offsets.iter().zip(counts.iter()) {
            data.extend_from_slice(bytes.get(*o as usize..(*o + *c) as usize).ok_or("truncated TIFF")?);
        }
        let spp = spp as usize;
        let count = (width * height) as usize;
        if data.len() < count * spp {
            return Err("truncated TIFF pixel data".to_string());
        }
        let mut pixels = Vec::with_capacity(count * 4);
        for px in data.chunks(spp).take(count) {
            let (mut r, mut g, mut b, a) = match spp {
                1 => (px[0], px[0], px[0], 255),
                2 => (px[0], px[0], px[0], px[1]),
                3 => (px[0], px[1], px[2], 255),
                _ => (px[0], px[1], px[2], px[3]),
            };
            if extra_associated && a != 0 && a != 255 {
                let un = |c: u8| ((c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8;
                r = un(r);
                g = un(g);
                b = un(b);
            }
            pixels.extend_from_slice(&[r, g, b, a]);
        }
        Ok(Image {
            pixels: PixelStore::Vec(pixels),
            dimensions: (width, height),
        })
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Image, String> {
        if bytes.starts_with(b"MM\0*") || bytes.starts_with(b"II*\0") {
            return Self::from_tiff_bytes(bytes);
        }
        let len: c_int = bytes.len().try_into().unwrap();

        let mut x: c_int = 0;
        let mut y: c_int = 0;
        let mut _channels_in_file: c_int = 0;

        // TODO: we're currently assuming this is sRGB, can we check somehow?

        let pixels = unsafe {
            // stb_image's support for CgBI images is a bit incomplete:
            // - If we don't ask it to "convert to RGB" for us, it will load
            //   CgBI PNGs in BGR and normal PNGs as RGB, with no way to
            //   distinguish them!
            // - If we don't ask it to "unpremultiply" for us, it won't do the
            //   RGB conversion.
            // So this is the only correct way to use it. :(
            stbi_convert_iphone_png_to_rgb(1);
            stbi_set_unpremultiply_on_load(1);
            stbi_load_from_memory(
                bytes.as_ptr(),
                len,
                &mut x,
                &mut y,
                &mut _channels_in_file,
                4,
            )
        };
        if pixels.is_null() {
            let reason = unsafe { CStr::from_ptr(stbi_failure_reason()) };
            return Err(reason.to_str().unwrap().to_string());
        }

        let width: u32 = x.try_into().unwrap();
        let height: u32 = y.try_into().unwrap();

        // (Un-un-)premultiply pixels to match iPhone OS's image loading.
        if bytes.starts_with(&PNG_MAGIC_NUMBER) {
            let len = width as usize * height as usize * 4;
            let pixels = unsafe { std::slice::from_raw_parts_mut(pixels, len) };
            let mut i = 0;
            while i < pixels.len() {
                let a = pixels[i + 3] as f32 / 255.0;
                pixels[i] = (pixels[i] as f32 * a) as u8;
                pixels[i + 1] = (pixels[i + 1] as f32 * a) as u8;
                pixels[i + 2] = (pixels[i + 2] as f32 * a) as u8;
                i += 4;
            }
        }

        Ok(Image {
            pixels: PixelStore::StbImage(pixels),
            dimensions: (width, height),
        })
    }

    /// TODO: This shouldn't really exist, it's a workaround for `CGImage`
    /// relying on this type and should be removed once it can be refactored.
    pub fn from_pixel_vec(pixels: Vec<u8>, dimensions: (u32, u32)) -> Image {
        assert!(dimensions.0 as usize * 4 * dimensions.1 as usize == pixels.len());
        Image {
            pixels: PixelStore::Vec(pixels),
            dimensions,
        }
    }

    pub fn dimensions(&self) -> (u32, u32) {
        self.dimensions
    }

    /// Get image data as bytes (8 bits per channel sRGB RGBA with premultiplied
    /// alpha). Rows are in top-to-bottom order.
    pub fn pixels(&self) -> &[u8] {
        match self.pixels {
            PixelStore::Vec(ref vec) => vec,
            PixelStore::StbImage(ptr) => unsafe {
                std::slice::from_raw_parts(
                    ptr,
                    self.dimensions.0 as usize * self.dimensions.1 as usize * 4,
                )
            },
        }
    }

    fn pixels_mut(&mut self) -> &mut [u8] {
        match self.pixels {
            PixelStore::Vec(ref mut vec) => vec,
            PixelStore::StbImage(ptr) => unsafe {
                std::slice::from_raw_parts_mut(
                    ptr,
                    self.dimensions.0 as usize * self.dimensions.1 as usize * 4,
                )
            },
        }
    }

    /// Get value of a pixel as linear RGBA (not sRGB!) with premultiplied
    /// alpha. 0 on the y axis is the top of the image.
    ///
    /// Returns [None] if `at` is out-of-bounds.
    pub fn get_pixel(&self, at: (i32, i32)) -> Option<(f32, f32, f32, f32)> {
        let (x, y) = at;
        let (x_usize, y_usize) = (x as usize, y as usize);
        let (width, height) = self.dimensions;
        let (width, height) = (width as usize, height as usize);
        if x >= 0 && x_usize < width && y >= 0 && y_usize < height {
            let rgba = &self.pixels()[y_usize * width * 4 + x_usize * 4..][..4];
            let [r, g, b, a]: [u8; 4] = rgba.try_into().unwrap();
            Some((
                gamma_decode(r as f32 / 255.0),
                gamma_decode(g as f32 / 255.0),
                gamma_decode(b as f32 / 255.0),
                a as f32 / 255.0, // alpha is linear
            ))
        } else {
            None
        }
    }

    // TODO: Eventually this should be in Core Animation instead?
    /// Modify the image to mask it with one to four anti-aliased rounded
    /// corners, and add sheen if desired.
    pub fn round_corners(&mut self, radius: f32, four_corners: bool, add_sheen: bool) {
        let (width, height) = self.dimensions();
        let (w_usize, h_usize) = (width as usize, height as usize);
        let (w, h) = (width as f32, height as f32);

        let (right_corners_begin, bottom_corners_begin) = if four_corners {
            (w - radius - 1.0, h - radius - 1.0)
        } else {
            (f32::INFINITY, f32::INFINITY)
        };
        let (sheen_center_x, sheen_center_y, sheen_radius) = if add_sheen {
            (w / 2.0, -(h / 2.0), h)
        } else {
            (f32::INFINITY, f32::INFINITY, 0.0)
        };
        for y_usize in 0..h_usize {
            for x_usize in 0..w_usize {
                let x = x_usize as f32;
                let y = y_usize as f32;

                let corner_x = (radius - x).max(x - right_corners_begin);
                let corner_y = (radius - y).max(y - bottom_corners_begin);
                let corner_circle_distance = corner_x.hypot(corner_y);
                let x_edge_distance = (x).min(w - x - 1.0);
                let y_edge_distance = (y).min(h - y - 1.0);
                let actual_corner_distance = x_edge_distance.hypot(y_edge_distance);
                let rounded_edge_distance = x_edge_distance
                    .min(y_edge_distance)
                    .min((radius.max(actual_corner_distance) - corner_circle_distance).max(0.0));
                let icon_opacity = if corner_x > 0.0 && corner_y > 0.0 {
                    // Bad approximation of the pixel coverage of a filled arc.
                    let distance = (corner_circle_distance - radius).clamp(0.0, 1.0);
                    let area = distance * distance;
                    1.0 - area
                } else {
                    1.0
                };
                let sheen_opacity = {
                    let distance = (x - sheen_center_x).hypot(y - sheen_center_y);
                    // Bad approximation of the pixel coverage of a filled arc.
                    let distance = (distance - sheen_radius).clamp(0.0, 1.0);
                    let area = distance * distance;
                    // The sheen is stronger at the top and close to edges.
                    let taper = (1.0
                        - ((y / h).sqrt() / 2.0)
                        - (rounded_edge_distance / (h).hypot(w)).sqrt() / 2.0)
                        * 0.75;
                    // There is also extra shinyness around the rim.
                    (1.0 - area) * taper
                };
                let rgba = &mut self.pixels_mut()[y_usize * w_usize * 4 + x_usize * 4..][..4];
                for channel in rgba.iter_mut() {
                    *channel = (*channel as f32 * icon_opacity * (1.0 - sheen_opacity)
                        + 255.0 * icon_opacity * sheen_opacity)
                        as u8;
                }
            }
        }
    }
}

impl Clone for Image {
    fn clone(&self) -> Image {
        // Note: implicitly converts pixel storage from StbImage to Vec
        // (if needed)
        Image::from_pixel_vec(self.pixels().to_vec(), self.dimensions)
    }
}

impl Drop for Image {
    fn drop(&mut self) {
        match self.pixels {
            PixelStore::StbImage(ptr) => unsafe { stbi_image_free(ptr.cast()) },
            PixelStore::Vec(_) => (),
        }
    }
}

/// Approximate implementation of sRGB gamma encoding.
pub fn gamma_encode(intensity: f32) -> f32 {
    // TODO: This doesn't implement the linear section near zero.
    intensity.powf(1.0 / 2.2)
}
/// Approximate implementation of sRGB gamma decoding.
pub fn gamma_decode(intensity: f32) -> f32 {
    // TODO: This doesn't implement the linear section near zero.
    intensity.powf(2.2)
}

/// Decodes Imagination Technologies' PVRTC texture compression format to
/// RGBA (8 bits per channel).
pub fn decode_pvrtc(pvrtc_data: &[u8], is_2bit: bool, width: u32, height: u32) -> Vec<u32> {
    // This formula is from the IMG_texture_compression_pvrtc extension spec.
    let expected_size = if is_2bit {
        (width.max(16) as usize * height.max(8) as usize * 2).div_ceil(8)
    } else {
        (width.max(8) as usize * height.max(8) as usize * 4).div_ceil(8)
    };
    assert!(pvrtc_data.len() == expected_size);

    let rgba8_word_count = width as usize * height as usize;
    let mut rgba8_data = Vec::with_capacity(rgba8_word_count);
    unsafe {
        let consumed_size = touchHLE_decompress_pvrtc(
            pvrtc_data.as_ptr() as *const _,
            is_2bit,
            width,
            height,
            // The interface says `uint8_t *` but the source seems to work with
            // 32-bit words, so using Vec<u32> seems more appropriate.
            rgba8_data.as_mut_ptr() as *mut u8,
        );
        assert_eq!(consumed_size as usize, expected_size);
        rgba8_data.set_len(rgba8_word_count);
    };
    rgba8_data
}
