// Embeds the icon, the application manifest (Per-Monitor V2 DPI awareness, common controls v6)
// and the version information into the Windows executable.
fn main() {
    println!("cargo:rerun-if-changed=assets/app.rc");
    println!("cargo:rerun-if-changed=assets/app.ico");
    println!("cargo:rerun-if-changed=assets/app.manifest");

    if !std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows") {
        return;
    }
    let result = embed_resource::compile("assets/app.rc", embed_resource::NONE);
    // A Windows host must be able to run the resource compiler. A host cross-compiling for
    // Windows may legitimately lack one (NotAttempted), but a script that actually failed must
    // never be swallowed: that would ship an exe with no icon, manifest or version information.
    let outcome = if cfg!(windows) {
        result.manifest_required()
    } else {
        result.manifest_optional()
    };
    outcome.unwrap_or_else(|error| panic!("could not compile Windows resources: {error}"));
}
