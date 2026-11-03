#[path = "src/ipc_catalog.rs"]
mod ipc_catalog;
fn main() {
    assert!(ipc_catalog::VIEWER_COMMANDS
        .iter()
        .all(|cmd| ipc_catalog::ALL_COMMANDS.contains(cmd)));
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    }
    println!("cargo:rerun-if-changed=src/ipc_catalog.rs");
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(ipc_catalog::ALL_COMMANDS)),
    )
    .expect("failed to build Tauri resources");
}
