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
            "dbghelp.dll",
            "shell32.dll",
            "gdi32.dll",
            "ole32.dll",
            "oleaut32.dll",
            "combase.dll",
            "comctl32.dll",
            "wtsapi32.dll",
            "version.dll",
            "psapi.dll",
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
        compile_icon_resource();
    }
}

fn find_rc_exe() -> Option<std::path::PathBuf> {
    if let Ok(p) = std::env::var("RC") {
        let path = std::path::PathBuf::from(p);
        if path.is_file() {
            return Some(path);
        }
    }
    let arch = match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("aarch64") => "arm64",
        Ok("x86") => "x86",
        _ => "x64",
    };
    let kits_bin = std::path::Path::new(r"C:\Program Files (x86)\Windows Kits\10\bin");
    if let Ok(entries) = std::fs::read_dir(kits_bin) {
        let mut versions: Vec<std::path::PathBuf> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.join(arch).join("rc.exe").is_file())
            .collect();
        versions.sort();
        if let Some(latest) = versions.pop() {
            return Some(latest.join(arch).join("rc.exe"));
        }
    }
    None
}

fn compile_icon_resource() {
    let manifest_dir =
        std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let rc_path = manifest_dir.join("assets").join("focus.rc");
    let ico_path = manifest_dir.join("assets").join("focus.ico");
    println!("cargo:rerun-if-changed={}", rc_path.display());
    println!("cargo:rerun-if-changed={}", ico_path.display());
    assert!(
        ico_path.is_file(),
        "missing {} (regenerate with `cargo run --example make_icon`)",
        ico_path.display()
    );

    let res_path = out_dir.join("focus_icon.res");
    let rc_exe = find_rc_exe().unwrap_or_else(|| std::path::PathBuf::from("rc.exe"));
    let status = std::process::Command::new(&rc_exe)
        .arg("/nologo")
        .arg("/fo")
        .arg(&res_path)
        .arg(&rc_path)
        .status()
        .unwrap_or_else(|e| panic!("failed to run {}: {e}", rc_exe.display()));
    assert!(
        status.success(),
        "rc.exe failed compiling {} with status {status}",
        rc_path.display()
    );
    println!("cargo:rustc-link-arg-bin=focus={}", res_path.display());
}
