use std::env;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proto_root = PathBuf::from("proto");

    let proto_file = proto_root.join("cognos.proto");

    println!(
        "cargo:rerun-if-changed={}",
        proto_file.display()
    );

    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .build_transport(true)
        .type_attribute(
            ".",
            "#[derive(serde::Serialize, serde::Deserialize)]",
        )
        .type_attribute(
            ".",
            "#[derive(Debug)]",
        )
        .out_dir(
            PathBuf::from(
                env::var("OUT_DIR")?
            )
        )
        .compile(
            &[proto_file],
            &[proto_root],
        )?;

    Ok(())
}