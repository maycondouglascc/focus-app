//! Generates `win32/assets/focus.ico` from the pure-Rust icon rasterizer.
//!
//! Run from `win32/`: `cargo run --example make_icon`

#[allow(dead_code)]
#[path = "../src/tray/icon.rs"]
mod icon;

fn main() {
    let out_dir = std::path::Path::new("assets");
    std::fs::create_dir_all(out_dir).expect("create assets/");
    let ico_path = out_dir.join("focus.ico");
    let data = icon::encode_ico(&[16, 20, 24, 32, 40, 48, 256]);
    std::fs::write(&ico_path, &data).expect("write assets/focus.ico");
    println!(
        "wrote {} ({} bytes, 7 frames: 16, 20, 24, 32, 40, 48, 256)",
        ico_path.display(),
        data.len()
    );
}
