//! Tile colours that stay the same between launches and exports.

/// FNV-1a, 64-bit: tiny, stable across platforms and Rust versions.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3))
}

/// sRGB colour for a tile, from its folder and file name. Same saturation and
/// value as the Python version (HSV 180/255, 210/255), but the hue no longer
/// changes on every launch, and two same-named files in different packs get
/// different colours.
pub fn tile_color(folder_name: &str, file_name: &str) -> [u8; 3] {
    let key = format!("{}/{}", folder_name.to_lowercase(), file_name.to_lowercase());
    let hue = (fnv1a(key.as_bytes()) % 360) as f32;
    hsv_to_rgb(hue, 180.0 / 255.0, 210.0 / 255.0)
}

pub fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [u8; 3] {
    let c = v * s;
    let hp = (h.rem_euclid(360.0)) / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r, g, b) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    [r, g, b].map(|ch| ((ch + m) * 255.0).round() as u8)
}

/// Relative luminance in 0..=255, for picking black or white label text.
pub fn luminance(rgb: [u8; 3]) -> f32 {
    0.299 * f32::from(rgb[0]) + 0.587 * f32::from(rgb[1]) + 0.114 * f32::from(rgb[2])
}

#[cfg(test)]
mod tests {
    use super::*;

    const PINNED: [u8; 3] = [210, 62, 198];

    #[test]
    fn colours_are_stable() {
        // Pinned: a change here changes every user's tile colours
        assert_eq!(tile_color("OpenForge", "floor_2x2.stl"), tile_color("openforge", "FLOOR_2X2.STL"));
        assert_eq!(tile_color("OpenForge", "floor_2x2.stl"), PINNED);
        assert_ne!(tile_color("OpenForge", "floor.stl"), tile_color("Dragonlock", "floor.stl"));
    }

    #[test]
    fn hsv_matches_qt() {
        // QColor.fromHsv(0, 180, 210) is (210, 62, 62)
        assert_eq!(hsv_to_rgb(0.0, 180.0 / 255.0, 210.0 / 255.0), [210, 62, 62]);
        assert_eq!(hsv_to_rgb(120.0, 180.0 / 255.0, 210.0 / 255.0), [62, 210, 62]);
    }
}
