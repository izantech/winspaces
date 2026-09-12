fn main() {
    println!("cargo:rerun-if-changed=winspaces.rc");
    println!("cargo:rerun-if-changed=../../assets/winspaces.ico");
    embed_resource::compile("winspaces.rc", embed_resource::NONE)
        .manifest_optional()
        .unwrap();
}
