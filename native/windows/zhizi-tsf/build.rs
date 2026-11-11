fn main() {
    embed_resource::compile("resources/zhizi.rc", embed_resource::NONE)
        .manifest_required()
        .unwrap();
}
