use std::io::Read;
use std::path::Path;

fn main() {
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let compiler = shaderc::Compiler::new().unwrap();
    let mut options = shaderc::CompileOptions::new().unwrap();

    for stage in ["vert", "frag"] {
        let src_path = format!("shaders/text.{}", stage);
        let out_path = format!("{}/text.{}.spv", out_dir, stage);

        println!("cargo:rerun-if-changed={}", src_path);

        let mut file = std::fs::File::open(&src_path).unwrap();
        let mut source = String::new();
        file.read_to_string(&mut source).unwrap();

        let kind = if stage == "vert" {
            shaderc::ShaderKind::Vertex
        } else {
            shaderc::ShaderKind::Fragment
        };

        let binary_result = compiler
            .compile_into_spirv(&source, kind, &src_path, "main", Some(&options))
            .unwrap();

        std::fs::write(out_path, binary_result.as_binary_u8()).unwrap();
    }
}
