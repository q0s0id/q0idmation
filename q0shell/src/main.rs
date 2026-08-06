use std::env;
use std::fs;
use std::path::PathBuf;

use q0shell::{print_help, ShellError, ShellHost};

fn main() {
    if let Err(err) = run_from_env() {
        eprintln!("q0shell: {err}");
        std::process::exit(1);
    }
}

fn run_from_env() -> Result<(), ShellError> {
    let mut args = env::args().skip(1);
    let mut script_path = None;
    let mut eval_chunks = Vec::new();
    let mut root = env::current_dir()?;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-e" | "--eval" => {
                let Some(value) = args.next() else {
                    return Err(ShellError::Usage(format!("missing value after {arg}")));
                };
                eval_chunks.push(value);
            }
            "--root" => {
                let Some(value) = args.next() else {
                    return Err(ShellError::Usage("missing value after --root".to_string()));
                };
                root = PathBuf::from(value);
            }
            "-h" | "--help" => {
                print_help();
                return Ok(());
            }
            _ if script_path.is_none() => script_path = Some(PathBuf::from(arg)),
            _ => {
                return Err(ShellError::Usage(format!("unexpected argument `{arg}`")));
            }
        }
    }

    if script_path.is_some() && !eval_chunks.is_empty() {
        return Err(ShellError::Usage(
            "use either a script path or -e/--eval chunks, not both".to_string(),
        ));
    }

    let source = if !eval_chunks.is_empty() {
        eval_chunks.join("\n")
    } else if let Some(script_path) = script_path {
        fs::read_to_string(&script_path)?
    } else {
        print_help();
        return Err(ShellError::Usage("script path required".to_string()));
    };

    let mut host = ShellHost::new(root)?;
    match host.run_source(&source) {
        Ok(lines) => {
            for line in lines {
                println!("{line}");
            }
            Ok(())
        }
        Err(ShellError::Diagnostics { count, log }) => {
            for line in log {
                eprintln!("{line}");
            }
            Err(ShellError::Diagnostics {
                count,
                log: Vec::new(),
            })
        }
        Err(err) => Err(err),
    }
}
