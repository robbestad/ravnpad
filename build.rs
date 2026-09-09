use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let svg_path = manifest.join("ravn-logo.svg");
    println!("cargo:rerun-if-changed={}", svg_path.display());

    let svg = fs::read(&svg_path).expect("kunne ikke lese ravn-logo.svg");
    let rgba_256 = rasterize(&svg, 256);
    save_png(&out.join("ravnpad-icon.png"), 256, &rgba_256);
    write_ico(&svg, &out.join("ravnpad.ico"));

    if std::env::var("CARGO_CFG_TARGET_OS").unwrap() == "windows" {
        let mut res = winresource::WindowsResource::new();
        res.set_icon(out.join("ravnpad.ico").to_str().expect("ico-path"));
        res.set("ProductName", "RavnPad");
        res.set("FileDescription", "RavnPad");
        res.set("CompanyName", "RavnPress");
        res.set("LegalCopyright", "RavnPress");
        res.set("InternalName", "ravnpad");
        res.set("OriginalFilename", "ravnpad.exe");
        res.compile().expect("kunne ikke bygge Windows-ressurser");
    }
}

fn rasterize(svg: &[u8], size: u32) -> Vec<u8> {
    let tree = resvg::usvg::Tree::from_data(svg, &resvg::usvg::Options::default())
        .expect("ugyldig ravn-logo.svg");
    let mut pixmap = resvg::tiny_skia::Pixmap::new(size, size).expect("pixmap");
    let svg_size = tree.size();
    let pad = 0.90;
    let scale = (size as f32 / svg_size.width().max(svg_size.height())) * pad;
    let tx = (size as f32 - svg_size.width() * scale) / 2.0;
    let ty = (size as f32 - svg_size.height() * scale) / 2.0;
    let transform = resvg::tiny_skia::Transform::from_row(scale, 0.0, 0.0, scale, tx, ty);
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    unpremultiply(pixmap.data())
}

fn unpremultiply(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    for px in data.chunks_exact(4) {
        let a = px[3] as u32;
        if a == 0 {
            out.extend_from_slice(&[0, 0, 0, 0]);
        } else if a == 255 {
            out.extend_from_slice(px);
        } else {
            out.push(((px[0] as u32 * 255 + a / 2) / a) as u8);
            out.push(((px[1] as u32 * 255 + a / 2) / a) as u8);
            out.push(((px[2] as u32 * 255 + a / 2) / a) as u8);
            out.push(px[3]);
        }
    }
    out
}

fn save_png(path: &Path, size: u32, rgba: &[u8]) {
    image::RgbaImage::from_raw(size, size, rgba.to_vec())
        .expect("png-buffer")
        .save(path)
        .expect("kunne ikke lagre png-ikon");
}

fn write_ico(svg: &[u8], path: &Path) {
    let mut dir = ico::IconDir::new(ico::ResourceType::Icon);
    for size in [16, 24, 32, 48, 64, 128, 256] {
        let rgba = rasterize(svg, size);
        let image = ico::IconImage::from_rgba_data(size, size, rgba);
        dir.add_entry(ico::IconDirEntry::encode(&image).expect("ico-entry"));
    }
    let file = fs::File::create(path).expect("ico-fil");
    dir.write(file).expect("kunne ikke skrive ico");
}
