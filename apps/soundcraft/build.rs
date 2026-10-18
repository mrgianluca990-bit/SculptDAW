//! Windows only: embed the app icon and version info (VERSIONINFO) into `soundcraft.exe`, so it
//! shows in Explorer, the taskbar, the Start menu and Alt-Tab.
//!
//! On every other target this does nothing. A missing resource compiler is a warning, so a
//! cross-compile from macOS or Linux still links, unless `SOUNDCRAFT_REQUIRE_WINRES=1` turns it
//! into an error (for release builds).

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../assets/sculpt-daw/sculpt-daw.ico");
    println!("cargo:rerun-if-env-changed=SOUNDCRAFT_REQUIRE_WINRES");
    // SCULPT_CORE19_APPLE_EVENT_BUILD: compile native AppKit open-document handler.
    // It does not replace winit's NSApplicationDelegate.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        use std::path::PathBuf;
        let arch = match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
            Ok("aarch64") => "arm64",
            Ok("x86_64") => "x86_64",
            _ => panic!("unknown Mac target"),
        };
        let out = PathBuf::from(std::env::var("OUT_DIR").expect("build OUT_DIR"));
        let object = out.join("sculpt_open_documents.o");
        let library = out.join("libsculpt_open_documents.a");
        println!("cargo:rerun-if-changed=objc/sculpt_open_documents.m");
        let ok = std::process::Command::new("clang")
            .args([
                "-x",
                "objective-c",
                "-fobjc-arc",
                "-fblocks",
                "-mmacosx-version-min=11.0",
                "-arch",
                arch,
                "-c",
                "objc/sculpt_open_documents.m",
                "-o",
            ])
            .arg(&object)
            .status()
            .expect("clang required for AppKit bridge");
        assert!(ok.success(), "compiling Finder document handler failed");
        let ok = std::process::Command::new("libtool").args(["-static", "-o"]).arg(&library).arg(&object).status().expect("macOS libtool required");
        assert!(ok.success(), "linking Finder document handler failed");
        // force_load: archive contains only a constructor, no Rust-referenced symbol.
        println!("cargo:rustc-link-arg=-Wl,-force_load,{}", library.display());
        println!("cargo:rustc-link-lib=framework=Cocoa");
        println!("cargo:rustc-link-lib=framework=Carbon");
    }
    // SCULPT_CORE20_MAC_SHIM: compile the native Finder Apple Event handler
    // only for the sculpt-daw binary. Does not affect the CLI or audio crates.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap_or_default());
        let obj = out.join("sculpt_open_documents.o");
        let target = std::env::var("TARGET").unwrap_or_default();
        let arch = if target.starts_with("aarch64") { "arm64" } else { "x86_64" };
        println!("cargo:rerun-if-changed=src/sculpt_open_documents.m");
        let status = std::process::Command::new("xcrun")
            .args(["clang", "-fobjc-arc", "-mmacosx-version-min=11.0", "-arch", arch, "-c", "src/sculpt_open_documents.m", "-o"])
            .arg(&obj)
            .status();
        if !matches!(&status, Ok(s) if s.success()) {
            panic!("Sculpt Finder native shim failed to compile: {status:?}");
        }
        println!("cargo:rustc-link-arg={}", obj.display());
        println!("cargo:rustc-link-lib=framework=Foundation");
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("../../assets/sculpt-daw/sculpt-daw.ico")
        .set("ProductName", "Sculpt DAW")
        .set("FileDescription", "Sculpt DAW digital audio workstation")
        .set("CompanyName", "Sculpt DAW contributors")
        .set("LegalCopyright", "Copyright (c) the SoundCraft authors. MIT OR Apache-2.0.")
        .set("OriginalFilename", "sculpt-daw.exe")
        .set("InternalName", "sculpt-daw");
    if let Err(e) = res.compile() {
        if std::env::var_os("SOUNDCRAFT_REQUIRE_WINRES").is_some() {
            eprintln!("embedding Windows resources failed: {e}");
            std::process::exit(1);
        }
        println!("cargo:warning=soundcraft.exe built without icon/version resources: {e}");
    }
}
