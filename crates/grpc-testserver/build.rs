// Use a vendored protoc so the test server codegens without a system protoc.
// Also emit a FileDescriptorSet so the server can serve gRPC reflection.
use std::path::PathBuf;

fn main() {
    let protoc = protoc_bin_vendored::protoc_bin_path().expect("vendored protoc binary");
    std::env::set_var("PROTOC", protoc);

    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    tonic_build::configure()
        .file_descriptor_set_path(out.join("echo_descriptor.bin"))
        .compile_protos(&["proto/echo.proto"], &["proto"])
        .expect("compile echo.proto");
}
