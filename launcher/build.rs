fn main() {
    println!("cargo:rerun-if-changed=../src-tauri/icons/icon.ico");
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        let mut res = tauri_winres::WindowsResource::new();
        res.set_icon("../src-tauri/icons/icon.ico");
        res.compile().expect("写入启动器 Windows 图标资源");
    }

    let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let dest = out_dir.join("payload.zip");
    println!("cargo:rerun-if-env-changed=TUXU_ZIP_PATH");
    match std::env::var("TUXU_ZIP_PATH") {
        Ok(path) => {
            println!("cargo:rerun-if-changed={path}");
            std::fs::copy(&path, &dest)
                .unwrap_or_else(|err| panic!("无法读取 TUXU_ZIP_PATH：{err}"));
        }
        Err(_) => {
            // Empty zip so `cargo test` can compile. The release script always sets TUXU_ZIP_PATH.
            const EMPTY_ZIP: &[u8] = &[
                0x50, 0x4b, 0x05, 0x06, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            ];
            std::fs::write(&dest, EMPTY_ZIP).expect("写入占位压缩包");
            println!("cargo:warning=未设置 TUXU_ZIP_PATH，使用占位压缩包，不能当正式发布");
        }
    }
}
