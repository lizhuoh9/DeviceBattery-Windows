// Embeds the icon and the application manifest (Per-Monitor V2 DPI awareness,
// common controls v6) into the Windows executable.
fn main() {
    println!("cargo:rerun-if-changed=assets/app.rc");
    println!("cargo:rerun-if-changed=assets/app.ico");
    println!("cargo:rerun-if-changed=assets/app.manifest");

    let targets_windows = std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows");
    if !targets_windows {
        return;
    }
    let result = embed_resource::compile("assets/app.rc", embed_resource::NONE);
    // Only a Windows host has the resource compiler; `cargo check` from macOS skips this.
    if cfg!(windows) {
        result
            .manifest_required()
            .expect("could not compile Windows resources");
    }
}
