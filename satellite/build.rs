fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure().build_client(false).compile_protos(&["../proto/satellite.proto"], &["../proto"])?;
    println!("cargo:rerun-if-changed=../proto/satellite.proto");
    Ok(())
}
