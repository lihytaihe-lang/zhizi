fn main() {
    println!("cargo:rustc-link-arg=/DELAYLOAD:rime.dll");
    println!("cargo:rustc-link-lib=delayimp");
    embed_resource::compile("resources/zhizi.rc", embed_resource::NONE)
        .manifest_required()
        .unwrap();
}
