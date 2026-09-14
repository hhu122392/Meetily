fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rustc-link-arg-bin=moss-helper=/DEPENDENTLOADFLAG:0x800");
    }
}
