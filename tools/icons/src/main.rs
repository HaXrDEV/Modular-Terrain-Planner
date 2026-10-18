//! Renders the app icon from its sources in `assets/icons`:
//!
//! - `app_icon.svg`: the icon, for 48 px and up
//! - `app_icon_small.svg`: the same picture drawn for 16, 24 and 32 px
//! - `app_icon_mac.svg`: `app_icon.svg` on Apple's icon grid, for the `.icns`
//!
//! into what each platform needs:
//!
//! - `icon-<size>.png` for 16 to 1024 px (window icon, Linux, README)
//! - `app_icon.ico` for the Windows executable and installer
//! - `app_icon.icns` for the macOS app bundle
//!
//! Run `cargo run -p mtp-icons` after changing an SVG and commit the output.

use std::path::Path;

use resvg::{tiny_skia, usvg};

const SIZES: [u32; 9] = [16, 24, 32, 48, 64, 128, 256, 512, 1024];

/// Up to this size the small drawing is used.
const SMALL: u32 = 32;

/// The margin around the icon on Apple's grid: 100 of 1024 px on each side.
const MAC_MARGIN: f32 = 100.0 / 1024.0;

fn load(dir: &Path, name: &str) -> Result<usvg::Tree, String> {
    let svg = std::fs::read(dir.join(name)).map_err(|e| format!("{name}: {e}"))?;
    // So app_icon_mac.svg can use app_icon.svg as an image.
    let options = usvg::Options { resources_dir: Some(dir.to_path_buf()), ..usvg::Options::default() };
    usvg::Tree::from_data(&svg, &options).map_err(|e| format!("{name}: {e}"))
}

/// Draws `tree` into a `size` square, inset by `margin` of the size on each side.
fn render(tree: &usvg::Tree, size: u32, margin: f32) -> Result<tiny_skia::Pixmap, String> {
    let mut pixmap = tiny_skia::Pixmap::new(size, size).ok_or("bad size")?;
    let inset = size as f32 * margin;
    let scale = (size as f32 - 2.0 * inset) / tree.size().width().max(tree.size().height());
    let transform = tiny_skia::Transform::from_scale(scale, scale).post_translate(inset, inset);
    resvg::render(tree, transform, &mut pixmap.as_mut());
    Ok(pixmap)
}

/// Straight (not premultiplied) RGBA, which ICO and ICNS expect.
fn unpremultiplied(pixmap: &tiny_skia::Pixmap) -> Vec<u8> {
    pixmap
        .pixels()
        .iter()
        .flat_map(|p| {
            let c = p.demultiply();
            [c.red(), c.green(), c.blue(), c.alpha()]
        })
        .collect()
}

fn main() -> Result<(), String> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/icons");
    let large = load(&dir, "app_icon.svg")?;
    let small = load(&dir, "app_icon_small.svg")?;
    let mac = load(&dir, "app_icon_mac.svg")?;

    let mut ico = ico::IconDir::new(ico::ResourceType::Icon);
    let mut icns = icns::IconFamily::new();
    for size in SIZES {
        let pixmap = render(if size <= SMALL { &small } else { &large }, size, 0.0)?;
        pixmap.save_png(dir.join(format!("icon-{size}.png"))).map_err(|e| e.to_string())?;
        if size <= 256 {
            let image = ico::IconImage::from_rgba_data(size, size, unpremultiplied(&pixmap));
            ico.add_entry(ico::IconDirEntry::encode(&image).map_err(|e| e.to_string())?);
        }
        if [16, 32, 64, 128, 256, 512, 1024].contains(&size) {
            // The small drawing has no shadow, so it goes on the grid by margin alone.
            let pixmap = if size <= SMALL { render(&small, size, MAC_MARGIN)? } else { render(&mac, size, 0.0)? };
            let image = icns::Image::from_data(icns::PixelFormat::RGBA, size, size, unpremultiplied(&pixmap))
                .map_err(|e| e.to_string())?;
            icns.add_icon(&image).map_err(|e| e.to_string())?;
        }
    }
    let ico_file = std::fs::File::create(dir.join("app_icon.ico")).map_err(|e| e.to_string())?;
    ico.write(ico_file).map_err(|e| e.to_string())?;
    let icns_file = std::fs::File::create(dir.join("app_icon.icns")).map_err(|e| e.to_string())?;
    icns.write(icns_file).map_err(|e| e.to_string())?;
    println!("wrote icons to {}", dir.display());
    Ok(())
}
