use std::env;
use std::path::Path;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=shaders/text.vert");
    println!("cargo:rerun-if-changed=shaders/text.frag");

    let out_dir = env::var("OUT_DIR").unwrap();

    let shaders = [
        ("shaders/text.vert", "text.vert.spv"),
        ("shaders/text.frag", "text.frag.spv"),
    ];

    for (src, dest_name) in shaders {
        let dest = Path::new(&out_dir).join(dest_name);
        
        let status = Command::new("glslc")
            .arg(src)
            .arg("-o")
            .arg(&dest)
            .status()
            .expect("Failed to execute glslc. Make sure it is installed.");

        if !status.success() {
            panic!("glslc failed to compile {}", src);
        }
    }
}
