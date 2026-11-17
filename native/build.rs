fn main() {
    #[cfg(feature = "gui")]
    {
        println!("cargo:rerun-if-changed=ui");
        slint_build::compile_with_config(
            "ui/app.slint",
            slint_build::CompilerConfiguration::new().with_style("fluent-dark".into()),
        )
        .expect("Slint interface must compile");
        if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
            println!("cargo:rerun-if-changed=../assets/liteticker.ico");
            println!("cargo:rerun-if-changed=preview.rc");
            embed_resource::compile("preview.rc", embed_resource::NONE)
                .manifest_required()
                .unwrap();
        }
    }
}
