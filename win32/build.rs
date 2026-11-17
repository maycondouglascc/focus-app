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
    }
}
