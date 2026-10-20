use std::path::PathBuf;
use std::process::Command;

fn main() {
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_windows_resources();
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    compile_laser_shader();
    compile_helper();
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    if let Ok(output) = Command::new("xcode-select").arg("-p").output()
        && output.status.success()
        && let Ok(developer_dir) = String::from_utf8(output.stdout)
    {
        for runtime in ["swift-5.5/macosx", "swift/macosx"] {
            let swift_runtime = format!(
                "{}/Toolchains/XcodeDefault.xctoolchain/usr/lib/{runtime}",
                developer_dir.trim()
            );
            println!("cargo:rustc-link-arg=-Wl,-rpath,{swift_runtime}");
        }
    }
}

/// The app icon (Explorer, the taskbar, Alt+Tab, and each window's title bar icon) and version
/// info. GPUI's `windows-manifest` feature embeds the DPI-aware manifest, so none is added here.
#[cfg(windows)]
fn embed_windows_resources() {
    let manifest_dir = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let output_dir = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let icon = manifest_dir.join("assets/Scrawl.ico");
    println!("cargo:rerun-if-changed={}", icon.display());
    let version = std::env::var("CARGO_PKG_VERSION").unwrap();
    let numeric = version
        .split(['.', '-'])
        .take(3)
        .map(|part| part.parse::<u16>().unwrap_or(0).to_string())
        .chain(std::iter::repeat("0".to_owned()))
        .take(4)
        .collect::<Vec<_>>()
        .join(",");
    let script = format!(
        r#"#include <winver.h>
1 ICON "{icon}"
1 VERSIONINFO
FILEVERSION {numeric}
PRODUCTVERSION {numeric}
FILEOS VOS_NT_WINDOWS32
FILETYPE VFT_APP
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "FileDescription", "Scrawl"
      VALUE "ProductName", "Scrawl"
      VALUE "FileVersion", "{version}"
      VALUE "ProductVersion", "{version}"
      VALUE "OriginalFilename", "scrawl.exe"
      VALUE "InternalName", "scrawl"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#,
        icon = icon.display().to_string().replace('\\', "/")
    );
    let resource = output_dir.join("scrawl.rc");
    std::fs::write(&resource, script).expect("write the Windows resource script");
    embed_resource::compile(&resource, embed_resource::NONE)
        .manifest_optional()
        .expect("compile the Windows resources");
}

fn compile_laser_shader() {
    for (src_rel, name) in [
        ("src/laser_overlay.metal", "laser_overlay"),
        ("src/native/pill/pill.metal", "pill"),
    ] {
        compile_metal_shader(src_rel, name);
    }
}

fn compile_metal_shader(src_rel: &str, name: &str) {
    let manifest_dir = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let output_dir = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let source = manifest_dir.join(src_rel);
    let air = output_dir.join(format!("{name}.air"));
    let library = output_dir.join(format!("{name}.metallib"));
    println!("cargo:rerun-if-changed={}", source.display());

    let metal = Command::new("xcrun")
        .args(["--sdk", "macosx", "metal", "-c", "-mmacosx-version-min=26.0"])
        .arg(&source)
        .arg("-o")
        .arg(&air)
        .status()
        .expect("xcrun metal is required to build Metal shaders");
    assert!(metal.success(), "Metal shader compilation failed for {src_rel}");

    let metallib = Command::new("xcrun")
        .args(["--sdk", "macosx", "metallib"])
        .arg(&air)
        .arg("-o")
        .arg(&library)
        .status()
        .expect("xcrun metallib is required to build Metal shaders");
    assert!(metallib.success(), "Metal library creation failed for {src_rel}");
}

/// Compiles the SwiftUI capsule's `[[stitchable]]` shaders (`helper/PillShaders.metal`) and
/// embeds the metallib in the helper as base64 (`PillShaderData.swift`), so the one binary
/// needs no resource file beside it.
fn embed_pill_shaders(helper_dir: &std::path::Path, out_dir: &std::path::Path) -> PathBuf {
    let source = helper_dir.join("PillShaders.metal");
    println!("cargo:rerun-if-changed={}", source.display());
    let air = out_dir.join("PillShaders.air");
    let lib = out_dir.join("PillShaders.metallib");
    let ok = Command::new("xcrun")
        .args(["--sdk", "macosx", "metal", "-c", "-mmacosx-version-min=26.0"])
        .arg(&source)
        .arg("-o")
        .arg(&air)
        .status()
        .is_ok_and(|s| s.success())
        && Command::new("xcrun")
            .args(["--sdk", "macosx", "metallib"])
            .arg(&air)
            .arg("-o")
            .arg(&lib)
            .status()
            .is_ok_and(|s| s.success());
    assert!(ok, "PillShaders.metal failed to compile");
    let bytes = std::fs::read(&lib).expect("read PillShaders.metallib");
    let swift = out_dir.join("PillShaderData.swift");
    std::fs::write(
        &swift,
        format!(
            "// Generated by build.rs from helper/PillShaders.metal.\nlet pillShaderLibraryBase64 = \"{}\"\n",
            base64(&bytes)
        ),
    )
    .expect("write PillShaderData.swift");
    swift
}

fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(T[(n >> (18 - 6 * i)) as usize & 63] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Builds `scrawl-helper` (Swift: SpeechAnalyzer, iPhone capture, movie decoding) beside the
/// `scrawl` binary, where `helper::path` and `scripts/install-app.sh` look for it.
fn compile_helper() {
    let manifest_dir = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let helper_dir = manifest_dir.join("helper");
    println!("cargo:rerun-if-changed={}", helper_dir.display());
    let mut sources: Vec<PathBuf> = std::fs::read_dir(&helper_dir)
        .expect("helper/ is missing")
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "swift"))
        .collect();
    sources.sort();
    for source in &sources {
        println!("cargo:rerun-if-changed={}", source.display());
    }
    let plist = helper_dir.join("Info.plist");
    println!("cargo:rerun-if-changed={}", plist.display());
    // OUT_DIR is target/<profile>/build/scrawl-<hash>/out.
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    sources.push(embed_pill_shaders(&helper_dir, &out_dir));
    let profile_dir = out_dir
        .ancestors()
        .nth(3)
        .expect("Unexpected OUT_DIR layout");
    let arch = match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("aarch64") => "arm64",
        Ok(other) => other,
        Err(_) => "arm64",
    }
    .to_owned();
    let built = out_dir.join("scrawl-helper");
    let status = Command::new("xcrun")
        .args(["swiftc", "-O", "-swift-version", "5", "-target"])
        .arg(format!("{arch}-apple-macos26.0"))
        .args(&sources)
        .arg("-o")
        .arg(&built)
        .args([
            "-Xlinker",
            "-sectcreate",
            "-Xlinker",
            "__TEXT",
            "-Xlinker",
            "__info_plist",
            "-Xlinker",
        ])
        .arg(&plist)
        .status()
        .expect("xcrun swiftc is required to build scrawl-helper");
    assert!(
        status.success(),
        "scrawl-helper failed to compile (needs the macOS 26 SDK)"
    );
    let dest = profile_dir.join("scrawl-helper");
    let _ = std::fs::remove_file(&dest);
    std::fs::copy(&built, dest).expect("Could not place scrawl-helper");
}
