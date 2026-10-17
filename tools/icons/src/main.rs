//! Renders the app icon from its one source, `assets/icons/app_icon.svg`,
//! into what each platform needs:
//!
//! - `icon-<size>.png` for 16 to 1024 px (window icon, Linux, README)
//! - `app_icon.ico` for the Windows executable and installer
//! - `app_icon.icns` for the macOS app bundle
//!
//! Run `cargo run -p mtp-icons` after changing the SVG and commit the output.

use std::path::Path;

const SIZES: [u32; 9] = [16, 24, 32, 48, 64, 128, 256, 512, 1024];

fn render(tree: &resvg::usvg::Tree, size: u32) -> Result<resvg::tiny_skia::Pixmap, String> {
    let mut pixmap = resvg::tiny_skia::Pixmap::new(size, size).ok_or("bad size")?;
    let scale = size as f32 / tree.size().width().max(tree.size().height());
    resvg::render(tree, resvg::tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    Ok(pixmap)
}

/// Straight (not premultiplied) RGBA, which ICO and ICNS expect.
fn unpremultiplied(pixmap: &resvg::tiny_skia::Pixmap) -> Vec<u8> {
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
    let svg = std::fs::read(dir.join("app_icon.svg")).map_err(|e| e.to_string())?;
    let tree = resvg::usvg::Tree::from_data(&svg, &resvg::usvg::Options::default()).map_err(|e| e.to_string())?;

    let mut ico = ico::IconDir::new(ico::ResourceType::Icon);
    let mut icns = icns::IconFamily::new();
    for size in SIZES {
        let pixmap = render(&tree, size)?;
        pixmap.save_png(dir.join(format!("icon-{size}.png"))).map_err(|e| e.to_string())?;
        let rgba = unpremultiplied(&pixmap);
        if size <= 256 {
            let image = ico::IconImage::from_rgba_data(size, size, rgba.clone());
            ico.add_entry(ico::IconDirEntry::encode(&image).map_err(|e| e.to_string())?);
        }
        if [16, 32, 64, 128, 256, 512, 1024].contains(&size) {
            let image = icns::Image::from_data(icns::PixelFormat::RGBA, size, size, rgba).map_err(|e| e.to_string())?;
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
