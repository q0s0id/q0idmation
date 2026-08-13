use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};

use q0s_format::q0lang::runtime::{Runtime, RuntimeAction, RuntimeValue};

pub fn print_help() {
    println!("q0shell - q0lang command-line host");
    println!("usage: q0shell <script.q0lang|script.q0l> [--root DIR]");
    println!("       q0shell -e \"q0shell.print! \\\"hi\\\"\" [--root DIR]");
    println!("       q0shell -e \"line 1\" -e \"line 2\" [--root DIR]");
    println!("commands:");
    println!("  q0shell.print!  value[, value...]");
    println!("  q0shell.read!   \"path\"");
    println!("  q0shell.write!  \"path\", \"text\"");
    println!("  q0shell.append! \"path\", \"text\"");
    println!("  q0shell.mkdir!  \"path\"");
    println!("  q0shell.list!   \"path\"");
    println!("  q0shell.move!   \"from\", \"to\"");
    println!("  q0shell.delete! \"path\"");
}

pub struct ShellHost {
    root: PathBuf,
}

impl ShellHost {
    pub fn new(root: PathBuf) -> Result<Self, ShellError> {
        fs::create_dir_all(&root)?;
        Ok(Self {
            root: root.canonicalize()?,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn run_source(&mut self, source: &str) -> Result<Vec<String>, ShellError> {
        let mut log = Vec::new();
        let mut runtime = Runtime::new();
        let report = runtime.execute_source(source);
        if !report.diagnostics.is_empty() {
            for diagnostic in &report.diagnostics {
                log.push(format!("line {}: {}", diagnostic.line, diagnostic.message));
            }
            return Err(ShellError::Diagnostics {
                count: report.diagnostics.len(),
                log,
            });
        }

        for action in &report.actions {
            match action {
                RuntimeAction::ShellCommand { command, args } => {
                    self.apply(command, args, &mut log)?;
                }
                RuntimeAction::GoRun(_) | RuntimeAction::GoStop(_) => {
                    log.push("timeline actions are unavailable in q0shell".to_string());
                }
                RuntimeAction::RigSetPosition { .. }
                | RuntimeAction::RigSetValue { .. }
                | RuntimeAction::RigReset { .. }
                | RuntimeAction::RigSetPose { .. }
                | RuntimeAction::RigResetPose { .. } => {
                    log.push("rig actions are unavailable in q0shell".to_string());
                }
            }
        }
        Ok(log)
    }

    fn apply(
        &mut self,
        command: &str,
        args: &[RuntimeValue],
        log: &mut Vec<String>,
    ) -> Result<(), ShellError> {
        match command {
            "print" => {
                log.push(
                    args.iter()
                        .map(RuntimeValue::display_lossy)
                        .collect::<Vec<_>>()
                        .join(" "),
                );
                Ok(())
            }
            "read" => {
                let path = self.one_path(command, args)?;
                log.push(fs::read_to_string(path)?);
                Ok(())
            }
            "write" => {
                let (path, text) = self.path_and_text(command, args)?;
                ensure_parent(&path)?;
                fs::write(path, text)?;
                Ok(())
            }
            "append" => {
                let (path, text) = self.path_and_text(command, args)?;
                ensure_parent(&path)?;
                use std::io::Write;
                let mut file = fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)?;
                file.write_all(text.as_bytes())?;
                Ok(())
            }
            "mkdir" => {
                let path = self.one_path(command, args)?;
                fs::create_dir_all(path)?;
                Ok(())
            }
            "list" => {
                let path = if args.is_empty() {
                    self.root.clone()
                } else {
                    self.one_path(command, args)?
                };
                let mut entries = fs::read_dir(path)?
                    .map(|entry| {
                        entry.map(|entry| {
                            let name = entry.file_name().to_string_lossy().to_string();
                            if entry.path().is_dir() {
                                format!("{name}/")
                            } else {
                                name
                            }
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                entries.sort();
                for entry in entries {
                    log.push(entry);
                }
                Ok(())
            }
            "move" => {
                if args.len() != 2 {
                    return Err(ShellError::Arity {
                        command: command.to_string(),
                        expected: "from, to",
                        got: args.len(),
                    });
                }
                let from = self.resolve_arg(&args[0])?;
                let to = self.resolve_arg(&args[1])?;
                ensure_parent(&to)?;
                fs::rename(from, to)?;
                Ok(())
            }
            "delete" => {
                let path = self.one_path(command, args)?;
                if path == self.root {
                    return Err(ShellError::UnsafePath(
                        "refusing to delete q0shell root".into(),
                    ));
                }
                let meta = fs::metadata(&path)?;
                if meta.is_dir() {
                    fs::remove_dir_all(path)?;
                } else {
                    fs::remove_file(path)?;
                }
                Ok(())
            }
            other => Err(ShellError::UnknownCommand(other.to_string())),
        }
    }

    fn one_path(&self, command: &str, args: &[RuntimeValue]) -> Result<PathBuf, ShellError> {
        if args.len() != 1 {
            return Err(ShellError::Arity {
                command: command.to_string(),
                expected: "path",
                got: args.len(),
            });
        }
        self.resolve_arg(&args[0])
    }

    fn path_and_text(
        &self,
        command: &str,
        args: &[RuntimeValue],
    ) -> Result<(PathBuf, String), ShellError> {
        if args.len() != 2 {
            return Err(ShellError::Arity {
                command: command.to_string(),
                expected: "path, text",
                got: args.len(),
            });
        }
        Ok((self.resolve_arg(&args[0])?, args[1].display_lossy()))
    }

    pub fn resolve_arg(&self, arg: &RuntimeValue) -> Result<PathBuf, ShellError> {
        let raw = arg.display_lossy();
        let rel = Path::new(&raw);
        if rel.as_os_str().is_empty() {
            return Err(ShellError::UnsafePath("empty path".into()));
        }
        if rel.is_absolute() {
            return Err(ShellError::UnsafePath(format!(
                "absolute paths are outside q0shell root: {raw}"
            )));
        }
        let mut clean = PathBuf::new();
        for component in rel.components() {
            match component {
                Component::Normal(part) => clean.push(part),
                Component::CurDir => {}
                Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                    return Err(ShellError::UnsafePath(format!(
                        "path escapes q0shell root: {raw}"
                    )));
                }
            }
        }
        Ok(self.root.join(clean))
    }
}

fn ensure_parent(path: &Path) -> Result<(), ShellError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}

#[derive(Debug)]
pub enum ShellError {
    Io(std::io::Error),
    Usage(String),
    Diagnostics {
        count: usize,
        log: Vec<String>,
    },
    UnknownCommand(String),
    UnsafePath(String),
    Arity {
        command: String,
        expected: &'static str,
        got: usize,
    },
}

impl fmt::Display for ShellError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ShellError::Io(err) => write!(f, "{err}"),
            ShellError::Usage(msg) => write!(f, "{msg}"),
            ShellError::Diagnostics { count, .. } => write!(f, "script has {count} diagnostic(s)"),
            ShellError::UnknownCommand(command) => write!(f, "unknown q0shell command `{command}`"),
            ShellError::UnsafePath(msg) => write!(f, "{msg}"),
            ShellError::Arity {
                command,
                expected,
                got,
            } => write!(
                f,
                "q0shell.{command}! expects {expected}, got {got} argument(s)"
            ),
        }
    }
}

impl std::error::Error for ShellError {}

impl From<std::io::Error> for ShellError {
    fn from(value: std::io::Error) -> Self {
        ShellError::Io(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn write_move_read_delete_stays_inside_root() {
        let root = unique_test_dir();
        let mut host = ShellHost::new(root.clone()).expect("host");
        host.run_source(
            r#"q0shell.mkdir! "box"
q0shell.write! "box/a.txt", "Q0"
q0shell.append! "box/a.txt", "S"
q0shell.move! "box/a.txt", "box/b.txt"
"#,
        )
        .expect("script");

        assert_eq!(
            fs::read_to_string(root.join("box").join("b.txt")).expect("read"),
            "Q0S"
        );

        host.run_source(r#"q0shell.delete! "box""#)
            .expect("delete dir");
        assert!(!root.join("box").exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_parent_dir_escape() {
        let root = unique_test_dir();
        let host = ShellHost::new(root.clone()).expect("host");
        let err = host
            .resolve_arg(&RuntimeValue::String("../outside.txt".to_string()))
            .expect_err("must reject ..");
        assert!(err.to_string().contains("escapes q0shell root"));
        let _ = fs::remove_dir_all(root);
    }

    fn unique_test_dir() -> PathBuf {
        let id = format!(
            "q0shell-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        );
        env::temp_dir().join(id)
    }
}
