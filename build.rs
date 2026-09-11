use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let logo_path = manifest.join("ravn-logo.png");
    println!("cargo:rerun-if-changed={}", logo_path.display());

    let src = image::open(&logo_path)
        .expect("kunne ikke lese ravn-logo.png")
        .to_rgba8();
    save_png(&out.join("ravnpad-icon.png"), 256, &resize_rgba(&src, 256));
    write_ico(&src, &out.join("ravnpad.ico"));
    let icns_path = out.join("ravnpad.icns");
    write_icns(&macos_icon(&src), &icns_path);
    // `OUT_DIR` is `target/{profile}/build/{pkg}-{hash}/out`. Copy next to the
    // binary so the macOS packaging step can pick it up without hashing.
    if let Some(profile_dir) = out.ancestors().nth(3) {
        let _ = fs::copy(&icns_path, profile_dir.join("ravnpad.icns"));
    }

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

// macOS draws app icons inside an ~824 pt squircle on a 1024 pt canvas.
// The source artwork is full-bleed, so center it on a transparent canvas;
// otherwise the Dock and Cmd-Tab icon look oversized next to normal apps.
fn macos_icon(src: &image::RgbaImage) -> image::RgbaImage {
    const FILL: f64 = 824.0 / 1024.0;
    let (w, h) = src.dimensions();
    let inner_w = (w as f64 * FILL).round() as u32;
    let inner_h = (h as f64 * FILL).round() as u32;
    let inner = image::imageops::resize(
        src,
        inner_w,
        inner_h,
        image::imageops::FilterType::Lanczos3,
    );
    let mut canvas = image::RgbaImage::from_pixel(w, h, image::Rgba([0, 0, 0, 0]));
    image::imageops::overlay(
        &mut canvas,
        &inner,
        ((w - inner_w) / 2).into(),
        ((h - inner_h) / 2).into(),
    );
    canvas
}

fn resize_rgba(src: &image::RgbaImage, size: u32) -> Vec<u8> {
    image::imageops::resize(src, size, size, image::imageops::FilterType::Lanczos3).into_raw()
}

fn save_png(path: &Path, size: u32, rgba: &[u8]) {
    image::RgbaImage::from_raw(size, size, rgba.to_vec())
        .expect("png-buffer")
        .save(path)
        .expect("kunne ikke lagre png-ikon");
}

fn write_icns(src: &image::RgbaImage, path: &Path) {
    let mut family = icns::IconFamily::new();
    for size in [16, 32, 48, 128, 256, 512, 1024] {
        let rgba = resize_rgba(src, size);
        let image = icns::Image::from_data(icns::PixelFormat::RGBA, size, size, rgba)
            .unwrap_or_else(|err| panic!("icns {size}x{size}: {err}"));
        family
            .add_icon(&image)
            .unwrap_or_else(|err| panic!("icns {size}x{size}: {err}"));
    }
    let file = fs::File::create(path).expect("icns-fil");
    family.write(file).expect("kunne ikke skrive icns");
}

fn write_ico(src: &image::RgbaImage, path: &Path) {
    let mut dir = ico::IconDir::new(ico::ResourceType::Icon);
    for size in [16, 24, 32, 48, 64, 128, 256] {
        let rgba = resize_rgba(src, size);
        let image = ico::IconImage::from_rgba_data(size, size, rgba);
        dir.add_entry(ico::IconDirEntry::encode(&image).expect("ico-entry"));
    }
    let file = fs::File::create(path).expect("ico-fil");
    dir.write(file).expect("kunne ikke skrive ico");
}
