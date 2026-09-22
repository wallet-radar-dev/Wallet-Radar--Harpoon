fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Кор говорить із сателітами, але сам сервером не є — серверний код не генеруємо.
    tonic_build::configure().build_server(false).compile_protos(&["../proto/satellite.proto"], &["../proto"])?;
    println!("cargo:rerun-if-changed=../proto/satellite.proto");
    Ok(())
}
