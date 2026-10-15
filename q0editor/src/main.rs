#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use q0editor::launch::Launch;

include!(concat!(env!("OUT_DIR"), "/window_icon.rs"));

fn main() -> eframe::Result<()> {
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    let no_vsync = arguments.iter().any(|argument| argument == "--no-vsync");
    let initial_path = arguments
        .into_iter()
        .find(|argument| argument != "--no-vsync")
        .map(std::path::PathBuf::from);
    let external_open_inbox =
        match q0editor::single_instance::claim_or_forward(initial_path.clone()) {
            q0editor::single_instance::LaunchDisposition::Primary(primary) => Some(primary.start()),
            q0editor::single_instance::LaunchDisposition::Forwarded => return Ok(()),
            q0editor::single_instance::LaunchDisposition::Secondary => None,
        };

    q0editor::launch::run(Launch {
        initial_path,
        vertical_sync: no_vsync.then_some(false),
        external_open_inbox,
        window_icon: Some(q0_window_icon()),
        ..Launch::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_window_uses_the_q0editor_icon() {
        let options = q0editor::launch::native_options(
            Some(q0_window_icon()),
            q0editor::launch::PREFERRED_MULTISAMPLING,
        );
        let icon = options.viewport.icon.expect("q0editor viewport icon");
        assert_eq!((icon.width, icon.height), (48, 48));
        assert_eq!(icon.rgba.len(), 48 * 48 * 4);
        assert!(icon.rgba.chunks_exact(4).any(|pixel| pixel[3] != 0));
    }
}
