fn main() {
    println!("cargo:rerun-if-changed=ui");
    if std::env::var_os("CARGO_FEATURE_GUI").is_some() {
        slint_build::compile("ui/app.slint").expect("Slint interface must compile");
    }
}
