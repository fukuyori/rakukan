fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=rakukan.rc");
    println!("cargo:rerun-if-changed=rakukan.ico");

    // ICO を DLL リソースとして埋め込む
    embed_resource::compile("rakukan.rc", embed_resource::NONE);
}
