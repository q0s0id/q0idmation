#[path = "../build_support/app_icon.rs"]
mod app_icon;

fn main() {
    app_icon::configure(
        "canripper.ico",
        "CanRipper",
        "q0idmation container and Flash inspector",
        "canripper.exe",
    );
}
