#[cfg(target_os = "windows")]
fn main() {
    println!("cargo:rerun-if-changed=assets/lazyterm.ico");
    println!("cargo:rerun-if-changed=../../logos/logoblackbackground.png");
    println!("cargo:rerun-if-changed=../../logos/logoblackbackground.svg");
    winresource::WindowsResource::new()
        .set_icon("assets/lazyterm.ico")
        .compile()
        .expect("compile Windows app resources");
}

#[cfg(not(target_os = "windows"))]
fn main() {}
