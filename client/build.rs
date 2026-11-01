fn main() {
    println!("cargo:rerun-if-changed=assets/antipunto.rc");
    println!("cargo:rerun-if-changed=assets/antipunto.manifest");
    println!("cargo:rerun-if-changed=assets/app.ico");
    embed_resource::compile("assets/antipunto.rc", embed_resource::NONE).manifest_required().unwrap();
}
