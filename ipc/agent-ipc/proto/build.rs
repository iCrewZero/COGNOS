/* Build script for cognos-ipc — compiles the canonical proto.

   Owner: iCrewZero
*/
fn main() {
    // The canonical proto lives at the workspace root ipc/grpc/proto/.
    // We compile from there so both cognos-ipc-grpc and cognos-ipc
    // generate the exact same types.
    let proto_path = "../../grpc/proto/cognos.proto";
    tonic_build::compile_protos(proto_path)
        .expect("failed to compile cognos.proto");
}
