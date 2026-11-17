fn main() {
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        // Delay-load UI, graphics, and accessibility DLLs on `focus.exe` so the
        // background monitor (`focus.exe` with no open window) stays under the F0 budget.
        for dll in [
            "d3d11.dll",
            "d2d1.dll",
            "dcomp.dll",
            "dwrite.dll",
            "UIAutomationCore.dll",
            "dwmapi.dll",
        ] {
            println!("cargo:rustc-link-arg-bin=focus=/DELAYLOAD:{dll}");
        }
        println!("cargo:rustc-link-arg-bin=focus=delayimp.lib");
        let manifest = std::path::PathBuf::from(
            std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"),
        )
        .join("focus.manifest");
        println!("cargo:rerun-if-changed=focus.manifest");
        println!("cargo:rustc-link-arg-bin=focus=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg-bin=focus=/MANIFESTINPUT:{}",
            manifest.display()
        );
    }
}
