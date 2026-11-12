use std::fs;
use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=shaders/text.vert");
    println!("cargo:rerun-if-changed=shaders/text.frag");

    let out_dir = std::env::var("OUT_DIR").unwrap();

    let compiler = shaderc::Compiler::new().expect("Failed to create shaderc compiler");
    let mut options = shaderc::CompileOptions::new().unwrap();

    options.set_target_env(
        shaderc::TargetEnv::Vulkan,
        shaderc::EnvVersion::Vulkan1_1 as u32,
    );

    for (src, kind, dest_name) in [
        ("shaders/text.vert", shaderc::ShaderKind::Vertex, "text.vert.spv"),
        ("shaders/text.frag", shaderc::ShaderKind::Fragment, "text.frag.spv"),
    ] {
        let source = fs::read_to_string(src).unwrap_or_else(|_| panic!("Missing {}", src));

        let binary_result = compiler
            .compile_into_spirv(&source, kind, src, "main", Some(&options))
            .unwrap_or_else(|e| panic!("Failed to compile {}: {}", src, e));

        let dest_path = Path::new(&out_dir).join(dest_name);
        fs::write(dest_path, binary_result.as_binary_u8()).unwrap();
    }
}