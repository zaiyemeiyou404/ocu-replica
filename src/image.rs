//! PNG encode + data-URL helpers (pure).

use base64::Engine as _;

/// Encode raw BGRA8 pixels (top-down, row stride = width*4) as a PNG.
pub fn encode_bgra_png(width: u32, height: u32, bgra: &[u8]) -> Result<Vec<u8>, String> {
    if width == 0 || height == 0 {
        return Err("empty image".into());
    }
    let expected = (width as usize).saturating_mul(height as usize).saturating_mul(4);
    if bgra.len() < expected {
        return Err(format!("pixel buffer too short: {} < {expected}", bgra.len()));
    }
    // Convert BGRA -> RGBA for the png crate.
    let mut rgba = Vec::with_capacity(expected);
    for px in bgra[..expected].chunks_exact(4) {
        rgba.push(px[2]);
        rgba.push(px[1]);
        rgba.push(px[0]);
        rgba.push(px[3]);
    }
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
        writer.write_image_data(&rgba).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

/// Nearest-neighbour resize of a BGRA buffer. Used when downscaling captures.
pub fn resize_bgra_nn(
    src_w: u32,
    src_h: u32,
    src: &[u8],
    dst_w: u32,
    dst_h: u32,
) -> Result<Vec<u8>, String> {
    if dst_w == 0 || dst_h == 0 {
        return Err("empty destination".into());
    }
    if src_w == dst_w && src_h == dst_h {
        let n = (src_w as usize) * (src_h as usize) * 4;
        return Ok(src[..n.min(src.len())].to_vec());
    }
    let mut out = vec![0u8; (dst_w as usize) * (dst_h as usize) * 4];
    for y in 0..dst_h {
        let sy = (y as u64 * src_h as u64 / dst_h as u64) as u32;
        for x in 0..dst_w {
            let sx = (x as u64 * src_w as u64 / dst_w as u64) as u32;
            let si = ((sy as usize) * (src_w as usize) + (sx as usize)) * 4;
            let di = ((y as usize) * (dst_w as usize) + (x as usize)) * 4;
            if si + 4 <= src.len() {
                out[di..di + 4].copy_from_slice(&src[si..si + 4]);
            }
        }
    }
    Ok(out)
}

pub fn png_data_url(png: &[u8]) -> String {
    let b64 = base64::engine::general_purpose::STANDARD.encode(png);
    format!("data:image/png;base64,{b64}")
}

/// Crop a BGRA buffer. `crop` is in source-pixel coordinates.
pub fn crop_bgra(
    src_w: u32,
    src_h: u32,
    src: &[u8],
    left: u32,
    top: u32,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, String> {
    if left.saturating_add(width) > src_w || top.saturating_add(height) > src_h {
        return Err("crop rectangle exceeds source".into());
    }
    let mut out = vec![0u8; (width as usize) * (height as usize) * 4];
    for row in 0..height {
        let src_off = (((top + row) as usize) * (src_w as usize) + (left as usize)) * 4;
        let dst_off = (row as usize) * (width as usize) * 4;
        let n = (width as usize) * 4;
        out[dst_off..dst_off + n].copy_from_slice(&src[src_off..src_off + n]);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_png() {
        // 2x2 red/blue checker.
        let bgra = [
            0u8, 0, 255, 255, 255, 0, 0, 255, //
            255, 0, 0, 255, 0, 0, 255, 255,
        ];
        let png = encode_bgra_png(2, 2, &bgra).unwrap();
        assert!(png.starts_with(&[0x89, b'P', b'N', b'G']));
        let url = png_data_url(&png);
        assert!(url.starts_with("data:image/png;base64,"));
    }

    #[test]
    fn resize_and_crop() {
        let src = vec![1u8; 4 * 4 * 4];
        let dst = resize_bgra_nn(4, 4, &src, 2, 2).unwrap();
        assert_eq!(dst.len(), 2 * 2 * 4);
        let cropped = crop_bgra(4, 4, &src, 1, 1, 2, 2).unwrap();
        assert_eq!(cropped.len(), 16);
        assert!(crop_bgra(4, 4, &src, 3, 3, 2, 2).is_err());
    }
}
