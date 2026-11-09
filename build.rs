// Embeds a Windows application manifest: modern-looking controls in the
// settings panel (Common Controls 6) and per-monitor DPI awareness.
fn main() {
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        embed_manifest::embed_manifest(embed_manifest::new_manifest("Stickman")).expect("unable to embed manifest");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
