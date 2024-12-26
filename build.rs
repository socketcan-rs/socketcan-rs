fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=build.rs");
    match std::env::var("CARGO_CFG_TARGET_OS") {
        Ok(val) if val == "linux" => Ok(()),
        _ => Err("Building for anything but Linux is not supported by socketcan".into()),
    }
}
