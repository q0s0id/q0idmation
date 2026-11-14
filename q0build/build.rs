#[path = "../build_support/app_icon.rs"]
mod app_icon;

fn main() {
    app_icon::configure(
        "q0build.ico",
        "Q0Build",
        "Packs q0s movies into Windows, Linux and Android apps",
        "q0build.exe",
    );
}
