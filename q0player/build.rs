#[path = "../build_support/app_icon.rs"]
mod app_icon;

fn main() {
    app_icon::configure(
        "q0player.ico",
        "q0player",
        "q0s movie player",
        "q0player.exe",
    );
}
