use std::fs;
use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=shaders/text.vert");
    println!("cargo:rerun-if-changed=shaders/text.frag");

    let out_dir = std::env::var("OUT_DIR").unwrap();

    compile_shader("shaders/text.vert", "text.vert.spv", naga::ShaderStage::Vertex, &out_dir);
    compile_shader("shaders/text.frag", "text.frag.spv", naga::ShaderStage::Fragment, &out_dir);
}

fn compile_shader(src: &str, dest_name: &str, stage: naga::ShaderStage, out_dir: &str) {
    let source = fs::read_to_string(src).unwrap_or_else(|_| panic!("Missing {}", src));

    let mut frontend = naga::front::glsl::Frontend::default();
    let options = naga::front::glsl::Options {
        stage,
        defines: Default::default(),
    };

    let module = frontend.parse(&options, &source)
        .unwrap_or_else(|e| panic!("Failed to parse {}: {:?}", src, e));

    let mut validator = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    );
    let info = validator.validate(&module)
        .unwrap_or_else(|e| panic!("Failed to validate {}: {:?}", src, e));

    let spv_options = naga::back::spv::Options::default();
    let mut writer = naga::back::spv::Writer::new(&spv_options).unwrap();
    let mut spv_words = Vec::new();
    writer.write(&module, &info, None, &mut spv_words)
        .unwrap_or_else(|e| panic!("Failed to write SPIR-V for {}: {:?}", src, e));

    let mut spv_bytes = Vec::with_capacity(spv_words.len() * 4);
    for word in spv_words {
        spv_bytes.extend_from_slice(&word.to_ne_bytes());
    }

    let dest_path = Path::new(out_dir).join(dest_name);
    fs::write(dest_path, spv_bytes).unwrap();
}