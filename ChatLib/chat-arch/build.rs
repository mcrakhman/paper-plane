fn main() -> Result<(), Box<dyn std::error::Error>> {
    let protobuf_out = "src/proto";
    std::fs::create_dir(&protobuf_out).ok();
    prost_build::Config::new()
        .out_dir(&protobuf_out)
        .compile_protos(&["src/proto/chat.proto"], &["src/proto"])?;
    Ok(())
}
