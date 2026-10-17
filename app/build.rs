//! Gives the Windows executable its icon and version details.

fn main() {
    println!("cargo:rerun-if-changed=../assets/icons/app_icon.ico");
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../assets/icons/app_icon.ico");
        res.set("FileDescription", "Modular Terrain Planner");
        res.set("ProductName", "Modular Terrain Planner");
        if let Err(e) = res.compile() {
            println!("cargo:warning=no icon in the executable: {e}");
        }
    }
}
