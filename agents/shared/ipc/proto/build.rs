use std::env;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proto_root = PathBuf::from("proto");
    let proto_file = proto_root.join("cognos.proto");

    println!("cargo:rerun-if-changed={}", proto_file.display());

    let out_dir = PathBuf::from(env::var("OUT_DIR")?);

    let descriptor_path = out_dir.join("cognos_descriptor.bin");

    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .build_transport(true)
        .file_descriptor_set_path(&descriptor_path)
        .type_attribute(".", "#[derive(serde::Serialize, serde::Deserialize)]")
        .out_dir(&out_dir)
        .compile(&[proto_file], &[proto_root])?;

    Ok(())
}
