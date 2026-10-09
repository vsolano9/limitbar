fn main() {
    println!("cargo:rerun-if-changed=res/limitbar.rc");
    println!("cargo:rerun-if-changed=res/limitbar.manifest");
    println!("cargo:rerun-if-changed=res/limitbar.ico");
    embed_resource::compile("res/limitbar.rc", embed_resource::NONE).manifest_required().unwrap();
}
