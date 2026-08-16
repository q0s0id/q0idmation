mod app;
mod icons;

use std::path::PathBuf;

use app::{apply_theme, CanRipperApp};
use canripper::{document::Document, extract::extract_document};

fn main() -> eframe::Result<()> {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if let Some(code) = run_cli_if_requested(&args) {
        std::process::exit(code);
    }

    let initial = args.first().map(PathBuf::from);
    let viewport = egui::ViewportBuilder::default()
        .with_inner_size([1180.0, 760.0])
        .with_min_inner_size([720.0, 420.0])
        .with_title("CanRipper");
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "CanRipper",
        options,
        Box::new(move |cc| {
            apply_theme(&cc.egui_ctx, CanRipperApp::default_theme());
            Box::new(CanRipperApp::new(initial))
        }),
    )
}

fn run_cli_if_requested(args: &[std::ffi::OsString]) -> Option<i32> {
    let command = args.first()?.to_string_lossy();
    match command.as_ref() {
        "inspect" => {
            let Some(input) = args.get(1) else {
                eprintln!("usage: canripper inspect <file>");
                return Some(2);
            };
            match Document::open(&PathBuf::from(input)) {
                Ok(document) => {
                    println!("{}", document.summary);
                    Some(0)
                }
                Err(error) => {
                    eprintln!("canripper: {error}");
                    Some(1)
                }
            }
        }
        "extract" => {
            let Some(input) = args.get(1) else {
                eprintln!("usage: canripper extract <file> [-o <directory>]");
                return Some(2);
            };
            let input = PathBuf::from(input);
            let output = match args.get(2).map(|value| value.to_string_lossy()) {
                Some(flag) if flag == "-o" || flag == "--output" => {
                    let Some(path) = args.get(3) else {
                        eprintln!("canripper: {flag} requires a directory");
                        return Some(2);
                    };
                    PathBuf::from(path)
                }
                Some(_) => {
                    eprintln!("usage: canripper extract <file> [-o <directory>]");
                    return Some(2);
                }
                None => default_output_path(&input),
            };
            match Document::open(&input).and_then(|document| {
                extract_document(&document, &output).map(|report| (document, report))
            }) {
                Ok((document, report)) => {
                    println!(
                        "extracted {} ({}) -> {} • {} files",
                        document.path.display(),
                        document.format.label(),
                        report.output.display(),
                        report.files_written
                    );
                    Some(0)
                }
                Err(error) => {
                    eprintln!("canripper: {error}");
                    Some(1)
                }
            }
        }
        "help" | "--help" | "-h" => {
            println!(
                "CanRipper\n\n  canripper                 open GUI\n  canripper <file>          open file in GUI\n  canripper inspect <file>  print structural summary\n  canripper extract <file> [-o <dir>]  extract resources"
            );
            Some(0)
        }
        _ => None,
    }
}

fn default_output_path(input: &std::path::Path) -> PathBuf {
    let parent = input.parent().unwrap_or_else(|| std::path::Path::new("."));
    let stem = input
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("ripped");
    parent.join(format!("{stem}_ripped"))
}
