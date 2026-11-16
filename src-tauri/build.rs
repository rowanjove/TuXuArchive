fn main() {
    for path in [
        "tauri.conf.json",
        "icons/32x32.png",
        "icons/64x64.png",
        "icons/128x128.png",
        "icons/128x128@2x.png",
        "icons/icon.png",
        "icons/icon.ico",
    ] {
        println!("cargo:rerun-if-changed={path}");
    }
    let mut attributes = tauri_build::Attributes::new();
    if std::env::var("CARGO_CFG_TARGET_FEATURE")
        .is_ok_and(|features| features.split(',').any(|feature| feature == "crt-static"))
    {
        // Tauri's static_vcruntime helper forces dynamic UCRT. With +crt-static,
        // let rustc and cc select the full static CRT instead (including SQLite).
        attributes = attributes
            .windows_attributes(tauri_build::WindowsAttributes::new().static_vc_runtime(false));
    }
    tauri_build::try_build(attributes).expect("构建 Tauri 资源与链接配置")
}
