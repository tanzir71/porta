fn main() {
    let attributes = tauri_build::Attributes::new();
    #[cfg(windows)]
    let attributes =
        attributes.windows_attributes(tauri_build::WindowsAttributes::new_without_app_manifest());
    #[cfg(windows)]
    embed_windows_manifest_for_every_artifact();

    tauri_build::try_build(attributes).expect("Tauri build setup should succeed");
}

#[cfg(windows)]
fn embed_windows_manifest_for_every_artifact() {
    let manifest = std::env::current_dir()
        .expect("the build should have a working directory")
        .join("windows-app-manifest.xml");

    println!("cargo:rerun-if-changed={}", manifest.display());
    println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
    println!("cargo:rustc-link-arg=/WX");
}
