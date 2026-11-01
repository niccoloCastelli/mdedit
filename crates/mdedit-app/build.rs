//! Embeds the app icon in the Windows executable (Explorer, taskbar, Alt+Tab).
//! Other platforms take the icon from the window (main.rs) and packaging/.

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed=../../assets/icons/mdedit.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../assets/icons/mdedit.ico");
        res.compile().expect("embed the Windows icon resource");
    }
}
