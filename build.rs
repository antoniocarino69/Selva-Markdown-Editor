use std::io;

fn main() -> io::Result<()> {
    println!("cargo:rerun-if-changed=icon.ico");
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap() == "windows" {
        let mut res = winres::WindowsResource::new();
        res.set_icon("icon.ico");
        res.compile()?;
    }
    Ok(())
}
