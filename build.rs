fn main() {
    println!("cargo:rerun-if-changed=assets/dsmaster.rc");
    println!("cargo:rerun-if-changed=assets/icon.ico");
    #[cfg(target_os = "windows")]
    {
        embed_resource::compile("assets/dsmaster.rc", embed_resource::NONE)
            .manifest_optional()
            .unwrap();
    }
}
