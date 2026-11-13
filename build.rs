fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=assets/icon.png");

    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() == "windows" {
        let mut res = winres::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.compile().expect("Failed to compile Windows resource");

        let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
        if target_env == "gnu" {
            let out_dir = std::env::var("OUT_DIR").unwrap();
            let obj_path = format!("{}/resource.o", out_dir.replace('\\', "/"));
            println!("cargo:rustc-link-arg={}", obj_path);
        }
    }
}
