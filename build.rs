// Windows resources for the exe:
// * version info (name, description, version): programs without it look
//   suspicious to antivirus heuristics,
// * an icon (a little stickman, drawn right here),
// * the application manifest: modern-looking controls in the settings panel
//   (Common Controls 6) and per-monitor DPI awareness.

const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <assemblyIdentity type="win32" name="Stickman" version="0.1.0.0"/>
  <dependency>
    <dependentAssembly>
      <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0" processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/>
    </dependentAssembly>
  </dependency>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/pm</dpiAware>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
    </windowsSettings>
  </application>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/>
    </application>
  </compatibility>
</assembly>
"#;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let ico = out.join("stickman.ico");
    std::fs::write(&ico, icon(48)).expect("unable to write icon");
    let mut res = winresource::WindowsResource::new();
    res.set_icon(ico.to_str().expect("icon path"))
        .set("FileDescription", "Stickman - a desktop pet")
        .set("ProductName", "Stickman")
        .set("CompanyName", "yoimdoingstuff")
        .set("LegalCopyright", "Open source: github.com/yoimdoingstuff/stickman")
        .set("OriginalFilename", "stickman.exe")
        .set("InternalName", "stickman")
        .set_manifest(MANIFEST);
    res.compile().expect("unable to compile Windows resources");
}

/// Distance from point p to the segment a-b.
fn dist(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 { (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
    let (cx, cy) = (a.0 + dx * t, a.1 + dy * t);
    ((p.0 - cx).powi(2) + (p.1 - cy).powi(2)).sqrt()
}

/// A .ico file with one 32-bit image of a waving stickman.
fn icon(size: u32) -> Vec<u8> {
    let k = size as f32;
    let at = |x: f32, y: f32| (x * k, y * k);
    let segments = [
        (at(0.5, 0.36), at(0.5, 0.64)),
        (at(0.5, 0.42), at(0.3, 0.6)),
        (at(0.5, 0.42), at(0.72, 0.3)),
        (at(0.72, 0.3), at(0.78, 0.12)),
        (at(0.32, 0.94), at(0.5, 0.64)),
        (at(0.5, 0.64), at(0.68, 0.94)),
    ];
    let head = at(0.5, 0.2);
    let (limb, head_r, rim) = (k * 0.048, k * 0.15, k * 0.035);
    // Pixels, bottom row first (as .ico wants), BGRA.
    let mut px = Vec::with_capacity((size * size * 4) as usize);
    for row in (0..size).rev() {
        for col in 0..size {
            let p = (col as f32 + 0.5, row as f32 + 0.5);
            // How far inside the body shape this pixel is (positive = inside).
            let mut inside = head_r - ((p.0 - head.0).powi(2) + (p.1 - head.1).powi(2)).sqrt();
            for (a, b) in segments {
                inside = inside.max(limb - dist(p, a, b));
            }
            let body = (inside + 0.5).clamp(0.0, 1.0);
            let outline = (inside + rim + 0.5).clamp(0.0, 1.0);
            // Dark body over a light rim, so it shows on any taskbar.
            let alpha = outline.max(body);
            let shade = 235.0 * (1.0 - body / alpha.max(1e-3)) + 20.0 * (body / alpha.max(1e-3));
            let v = shade.round() as u8;
            px.extend_from_slice(&[v, v, v, (alpha * 255.0).round() as u8]);
        }
    }
    let mask_row = size.div_ceil(32) * 4;
    let image_len = 40 + px.len() as u32 + mask_row * size;
    let mut out = Vec::new();
    // ICONDIR + one ICONDIRENTRY.
    out.extend_from_slice(&[0, 0, 1, 0, 1, 0]);
    out.extend_from_slice(&[size as u8, size as u8, 0, 0]);
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&image_len.to_le_bytes());
    out.extend_from_slice(&22u32.to_le_bytes());
    // BITMAPINFOHEADER (height doubled: colour + mask).
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(size as i32).to_le_bytes());
    out.extend_from_slice(&(size as i32 * 2).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&[0u8; 24]);
    out.extend_from_slice(&px);
    // AND mask (unused with alpha, but required).
    out.extend(std::iter::repeat(0u8).take((mask_row * size) as usize));
    out
}
