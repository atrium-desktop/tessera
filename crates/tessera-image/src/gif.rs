//! Pure-Rust GIF87a / GIF89a + LZW decoder.

use std::time::Duration;

use crate::{AnimFrame, ImageError};

pub(crate) fn decode_gif_frames(
    bytes: &[u8],
    max_edge: u32,
    max_alloc: u64,
) -> Result<(u32, u32, Vec<AnimFrame>), ImageError> {
    if bytes.len() < 13 || (&bytes[0..6] != b"GIF87a" && &bytes[0..6] != b"GIF89a") {
        return Err(ImageError::Decode("invalid GIF header".into()));
    }
    let canvas_w = u16::from_le_bytes([bytes[6], bytes[7]]) as u32;
    let canvas_h = u16::from_le_bytes([bytes[8], bytes[9]]) as u32;
    if canvas_w == 0 || canvas_h == 0 || canvas_w > max_edge || canvas_h > max_edge {
        return Err(ImageError::Dimensions(canvas_w, canvas_h));
    }
    let canvas_bytes = u64::from(canvas_w)
        .checked_mul(u64::from(canvas_h))
        .and_then(|p| p.checked_mul(4))
        .filter(|b| *b <= max_alloc)
        .ok_or(ImageError::Dimensions(canvas_w, canvas_h))?;
    let _ = canvas_bytes;

    let packed = bytes[10];
    let has_gct = (packed & 0x80) != 0;
    let gct_len = if has_gct {
        1usize << ((packed & 0x07) + 1)
    } else {
        0
    };
    let mut pos = 13usize;
    let gct = if has_gct {
        let end = pos + gct_len * 3;
        if end > bytes.len() {
            return Err(ImageError::Decode(
                "truncated GIF global color table".into(),
            ));
        }
        let table = &bytes[pos..end];
        pos = end;
        Some(table)
    } else {
        None
    };

    let mut frames = Vec::new();
    let mut delay_cs: u16 = 0;
    let mut transparent_index: Option<u8> = None;

    while pos < bytes.len() {
        let introducer = bytes[pos];
        pos += 1;
        match introducer {
            0x3B => break, // Trailer
            0x21 => {
                // Extension
                if pos >= bytes.len() {
                    break;
                }
                let label = bytes[pos];
                pos += 1;
                if label == 0xF9 {
                    // Graphic Control Extension
                    let block_len = *bytes.get(pos).unwrap_or(&0) as usize;
                    pos += 1;
                    if block_len >= 4 && pos + block_len <= bytes.len() {
                        let flags = bytes[pos];
                        delay_cs = u16::from_le_bytes([bytes[pos + 1], bytes[pos + 2]]);
                        transparent_index = if (flags & 0x01) != 0 {
                            Some(bytes[pos + 3])
                        } else {
                            None
                        };
                    }
                    pos = (pos + block_len).min(bytes.len());
                    pos = skip_sub_blocks(bytes, pos)?;
                } else {
                    pos = skip_sub_blocks(bytes, pos)?;
                }
            }
            0x2C => {
                // Image Descriptor
                if pos + 9 > bytes.len() {
                    return Err(ImageError::Decode("truncated GIF image descriptor".into()));
                }
                let left = u16::from_le_bytes([bytes[pos], bytes[pos + 1]]) as u32;
                let top = u16::from_le_bytes([bytes[pos + 2], bytes[pos + 3]]) as u32;
                let width = u16::from_le_bytes([bytes[pos + 4], bytes[pos + 5]]) as u32;
                let height = u16::from_le_bytes([bytes[pos + 6], bytes[pos + 7]]) as u32;
                let img_packed = bytes[pos + 8];
                pos += 9;

                if width > max_edge || height > max_edge {
                    return Err(ImageError::Dimensions(width, height));
                }

                let has_lct = (img_packed & 0x80) != 0;
                let interlaced = (img_packed & 0x40) != 0;
                let lct = if has_lct {
                    let len = 1usize << ((img_packed & 0x07) + 1);
                    let end = pos + len * 3;
                    if end > bytes.len() {
                        return Err(ImageError::Decode("truncated GIF local color table".into()));
                    }
                    let table = &bytes[pos..end];
                    pos = end;
                    Some(table)
                } else {
                    None
                };

                let color_table = lct.or(gct).ok_or_else(|| {
                    ImageError::Decode("GIF image has neither local nor global color table".into())
                })?;

                if pos >= bytes.len() {
                    return Err(ImageError::Decode(
                        "truncated GIF LZW minimum code size".into(),
                    ));
                }
                let min_code_size = bytes[pos];
                pos += 1;
                if !(2..=8).contains(&min_code_size) {
                    return Err(ImageError::Decode(format!(
                        "invalid GIF LZW code size {min_code_size}"
                    )));
                }

                let (lzw_data, next_pos) = read_sub_blocks(bytes, pos)?;
                pos = next_pos;

                let pixel_count = (width as usize)
                    .checked_mul(height as usize)
                    .ok_or(ImageError::Dimensions(width, height))?;
                let indices = decode_lzw(&lzw_data, min_code_size, pixel_count)?;

                let mut rgba = vec![0u8; pixel_count * 4];
                if interlaced {
                    let mut src_i = 0usize;
                    for &(start_row, step) in &[(0usize, 8usize), (4, 8), (2, 4), (1, 2)] {
                        let mut y = start_row;
                        while y < height as usize {
                            for x in 0..width as usize {
                                let idx = *indices.get(src_i).unwrap_or(&0);
                                src_i += 1;
                                write_gif_pixel(
                                    &mut rgba,
                                    (y * width as usize + x) * 4,
                                    idx,
                                    color_table,
                                    transparent_index,
                                );
                            }
                            y += step;
                        }
                    }
                } else {
                    for (i, &idx) in indices.iter().enumerate().take(pixel_count) {
                        write_gif_pixel(&mut rgba, i * 4, idx, color_table, transparent_index);
                    }
                }

                let delay = Duration::from_millis(u64::from(delay_cs) * 10);
                frames.push(AnimFrame {
                    left,
                    top,
                    width,
                    height,
                    rgba,
                    delay,
                });
                delay_cs = 0;
                transparent_index = None;
            }
            _ => {
                // Unknown block; stop cleanly if we already have frames.
                break;
            }
        }
    }

    if frames.is_empty() {
        return Err(ImageError::Decode("GIF contains no image frames".into()));
    }
    Ok((canvas_w, canvas_h, frames))
}

fn write_gif_pixel(
    rgba: &mut [u8],
    dst: usize,
    idx: u8,
    color_table: &[u8],
    transparent: Option<u8>,
) {
    if transparent == Some(idx) {
        rgba[dst..dst + 4].copy_from_slice(&[0, 0, 0, 0]);
        return;
    }
    let off = usize::from(idx) * 3;
    if off + 2 < color_table.len() {
        rgba[dst] = color_table[off];
        rgba[dst + 1] = color_table[off + 1];
        rgba[dst + 2] = color_table[off + 2];
        rgba[dst + 3] = 255;
    }
}

fn skip_sub_blocks(bytes: &[u8], mut pos: usize) -> Result<usize, ImageError> {
    while pos < bytes.len() {
        let len = bytes[pos] as usize;
        pos += 1;
        if len == 0 {
            return Ok(pos);
        }
        pos = pos
            .checked_add(len)
            .filter(|p| *p <= bytes.len())
            .ok_or_else(|| ImageError::Decode("truncated GIF sub-block".into()))?;
    }
    Ok(pos)
}

fn read_sub_blocks(bytes: &[u8], mut pos: usize) -> Result<(Vec<u8>, usize), ImageError> {
    let mut out = Vec::new();
    while pos < bytes.len() {
        let len = bytes[pos] as usize;
        pos += 1;
        if len == 0 {
            return Ok((out, pos));
        }
        let end = pos
            .checked_add(len)
            .filter(|p| *p <= bytes.len())
            .ok_or_else(|| ImageError::Decode("truncated GIF image sub-block".into()))?;
        out.extend_from_slice(&bytes[pos..end]);
        pos = end;
    }
    Ok((out, pos))
}

fn decode_lzw(
    data: &[u8],
    min_code_size: u8,
    expected_pixels: usize,
) -> Result<Vec<u8>, ImageError> {
    let clear_code = 1u16 << min_code_size;
    let eoi_code = clear_code + 1;

    let mut code_size = u32::from(min_code_size) + 1;
    let mut next_code = eoi_code + 1;
    let mut prefix = [0u16; 4096];
    let mut suffix = [0u8; 4096];
    let mut first_char = [0u8; 4096];
    for i in 0..clear_code {
        suffix[i as usize] = i as u8;
        first_char[i as usize] = i as u8;
    }

    let mut bit_buf = 0u32;
    let mut bit_count = 0u32;
    let mut byte_pos = 0usize;

    let mut out = Vec::with_capacity(expected_pixels);
    let mut stack = Vec::with_capacity(64);
    let mut prev_code: Option<u16> = None;

    loop {
        while bit_count < code_size && byte_pos < data.len() {
            bit_buf |= u32::from(data[byte_pos]) << bit_count;
            bit_count += 8;
            byte_pos += 1;
        }
        if bit_count < code_size {
            break;
        }
        let mask = (1u32 << code_size) - 1;
        let code = (bit_buf & mask) as u16;
        bit_buf >>= code_size;
        bit_count -= code_size;

        if code == clear_code {
            code_size = u32::from(min_code_size) + 1;
            next_code = eoi_code + 1;
            prev_code = None;
            continue;
        }
        if code == eoi_code {
            break;
        }

        let in_code = code;
        let mut cur = code;
        stack.clear();

        if cur >= next_code {
            let Some(p) = prev_code else {
                return Err(ImageError::Decode("corrupt GIF LZW stream".into()));
            };
            if cur > next_code {
                return Err(ImageError::Decode("invalid GIF LZW code".into()));
            }
            stack.push(first_char[p as usize]);
            cur = p;
        }

        while cur >= clear_code {
            stack.push(suffix[cur as usize]);
            cur = prefix[cur as usize];
            if stack.len() > 4096 {
                return Err(ImageError::Decode("GIF LZW cycle detected".into()));
            }
        }
        let first = cur as u8;
        stack.push(first);

        for &px in stack.iter().rev() {
            if out.len() < expected_pixels {
                out.push(px);
            }
        }

        if let Some(p) = prev_code
            && next_code < 4096
        {
            prefix[next_code as usize] = p;
            suffix[next_code as usize] = first;
            first_char[next_code as usize] = first_char[p as usize];
            next_code += 1;
            if next_code == (1u16 << code_size) && code_size < 12 {
                code_size += 1;
            }
        }
        prev_code = Some(in_code);
        if out.len() >= expected_pixels {
            break;
        }
    }

    Ok(out)
}
