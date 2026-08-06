pub fn configure(icon_name: &str, product_name: &str, description: &str, original_filename: &str) {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is always set");
    let icon = std::path::Path::new(&manifest)
        .join("..")
        .join("installer")
        .join("assets")
        .join(icon_name);

    println!("cargo:rerun-if-changed={}", icon.display());
    println!("cargo:rerun-if-changed=../build_support/app_icon.rs");
    println!("cargo:rerun-if-changed=build.rs");

    let icon_bytes = std::fs::read(&icon).unwrap_or_else(|error| {
        panic!(
            "{product_name} icon is required at {}: {error}. Run `python installer/build_assets.py`.",
            icon.display()
        )
    });
    let runtime_icon = q0icon::decode_ico_rgba(&icon_bytes, 48).unwrap_or_else(|error| {
        panic!(
            "failed to decode {} for the native window: {error}",
            icon.display()
        )
    });
    let out_dir = std::path::PathBuf::from(
        std::env::var_os("OUT_DIR").expect("OUT_DIR is always set for build scripts"),
    );
    std::fs::write(out_dir.join("window_icon.rgba"), &runtime_icon.rgba)
        .expect("failed to write native window icon pixels");
    let generated = format!(
        "fn q0_window_icon() -> egui::IconData {{\n    egui::IconData {{\n        rgba: include_bytes!(concat!(env!(\"OUT_DIR\"), \"/window_icon.rgba\")).to_vec(),\n        width: {},\n        height: {},\n    }}\n}}\n",
        runtime_icon.width, runtime_icon.height
    );
    std::fs::write(out_dir.join("window_icon.rs"), generated)
        .expect("failed to write native window icon module");

    #[cfg(windows)]
    embed_windows_resources(&icon, product_name, description, original_filename);
}

#[cfg(windows)]
fn embed_windows_resources(
    icon: &std::path::Path,
    product_name: &str,
    description: &str,
    original_filename: &str,
) {
    let mut resources = winres::WindowsResource::new();
    resources.set_icon(icon.to_str().expect("icon path is utf-8"));
    resources.set("ProductName", product_name);
    resources.set("FileDescription", description);
    resources.set("CompanyName", "q0s");
    resources.set("LegalCopyright", "(C) 2026 q0s");
    resources.set("OriginalFilename", original_filename);
    resources
        .compile()
        .unwrap_or_else(|error| panic!("winres compile failed for {product_name}: {error}"));
}
