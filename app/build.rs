fn main() {
    #[cfg(target_os = "windows")]
    {
        println!("cargo:rerun-if-changed=assets/telinha.ico");
        println!("cargo:rerun-if-changed=assets/telinha-live.ico");
        println!("cargo:rerun-if-changed=assets/windows.rc");
        embed_resource::compile("assets/windows.rc", embed_resource::NONE)
            .manifest_required()
            .expect("ícone Windows da telinha");
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rerun-if-changed=src/app_icon_macos.m");
        cc::Build::new()
            .file("src/app_icon_macos.m")
            .flag("-fobjc-arc")
            .compile("telinha_app_icon");
        println!("cargo:rustc-link-lib=framework=AppKit");
        // Load WebRTC's Objective-C categories (notably NSString+StdString).
        // A binary-only link argument avoids rebuilding every dependency.
        println!("cargo:rustc-link-arg=-ObjC");
    }
}
