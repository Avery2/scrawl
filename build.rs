use std::path::PathBuf;
use std::process::Command;

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    compile_laser_shader();
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

fn compile_laser_shader() {
    let manifest_dir = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let output_dir = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let source = manifest_dir.join("src/laser_overlay.metal");
    let air = output_dir.join("laser_overlay.air");
    let library = output_dir.join("laser_overlay.metallib");
    println!("cargo:rerun-if-changed={}", source.display());

    let metal = Command::new("xcrun")
        .args(["--sdk", "macosx", "metal", "-c"])
        .arg(&source)
        .arg("-o")
        .arg(&air)
        .status()
        .expect("xcrun metal is required to build the laser overlay shader");
    assert!(metal.success(), "Metal shader compilation failed");

    let metallib = Command::new("xcrun")
        .args(["--sdk", "macosx", "metallib"])
        .arg(&air)
        .arg("-o")
        .arg(&library)
        .status()
        .expect("xcrun metallib is required to build the laser overlay shader");
    assert!(metallib.success(), "Metal library creation failed");
}
