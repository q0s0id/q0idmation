#[path = "../build_support/app_icon.rs"]
mod app_icon;

fn main() {
    app_icon::configure(
        "q0editor.ico",
        "q0editor",
        "q0s vector animation editor",
        "q0editor.exe",
    );
}
