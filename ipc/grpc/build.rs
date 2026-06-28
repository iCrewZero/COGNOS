// COGNOS gRPC proto build script.
// Owner: iCrewZero — added rerun-if-changed so cargo only re-runs when the proto changes.
fn main() {
    println!("cargo:rerun-if-changed=proto/cognos.proto");
    tonic_build::compile_protos("proto/cognos.proto")
        .unwrap_or_else(|e| panic!("proto compilation failed: {e}"));
}