fn main() {
    // No WebView commands: only the trusted Rust session boundary calls this API.
    tauri_plugin::Builder::new(&[])
        .android_path("android")
        .build();
}
