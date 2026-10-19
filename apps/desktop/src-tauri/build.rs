fn main() {
    #[cfg(windows)]
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&["desktop_action", "describe_frame"]),
    ))
    .expect("build the desktop command manifest and bundled assets");
}
