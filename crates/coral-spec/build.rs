//! Build script for generated source-spec manifest protobuf types.

fn main() {
    let protoc = protoc_bin_vendored::protoc_bin_path().expect("vendored protoc");
    let mut config = prost_build::Config::new();
    config.protoc_executable(protoc);

    config
        .compile_protos(&["proto/coral/spec/v1/source.proto"], &["proto"])
        .expect("compile coral spec protobuf");
}
