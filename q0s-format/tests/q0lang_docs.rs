//! Every q0lang example in the language reference must parse and compile
//! without a diagnostic, so the documentation cannot drift from the language.

use std::path::{Path, PathBuf};

fn docs_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("docs")
        .join("q0lang")
}

fn q0lang_blocks(markdown: &str) -> Vec<(usize, String)> {
    let mut blocks = Vec::new();
    let mut current: Option<(usize, String)> = None;
    for (index, line) in markdown.lines().enumerate() {
        match &mut current {
            None if line.trim_start().starts_with("```q0lang") => {
                current = Some((index + 1, String::new()));
            }
            Some(_) if line.trim_start().starts_with("```") => {
                blocks.push(current.take().expect("inside a block"));
            }
            Some((_, source)) => {
                source.push_str(line);
                source.push('\n');
            }
            None => {}
        }
    }
    blocks
}

#[test]
fn every_reference_example_compiles_cleanly() {
    let mut checked = 0;
    let mut failures = Vec::new();
    let mut pages = std::fs::read_dir(docs_dir())
        .expect("docs/q0lang exists")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "md"))
        // The plans are design history and sketch syntax that was never built.
        .filter(|path| {
            !path
                .file_stem()
                .is_some_and(|stem| stem.to_string_lossy().ends_with("_plan"))
        })
        .collect::<Vec<_>>();
    pages.sort();
    for page in pages {
        let markdown = std::fs::read_to_string(&page).expect("readable page");
        for (line, source) in q0lang_blocks(&markdown) {
            checked += 1;
            let compiled = q0s_format::q0lang::compiler::compile_source(&source);
            if !compiled.diagnostics.is_empty() {
                failures.push(format!(
                    "{}:{line}: {:?}",
                    page.file_name().unwrap_or_default().to_string_lossy(),
                    compiled.diagnostics
                ));
            }
        }
    }
    let examples = docs_dir().join("examples");
    for entry in std::fs::read_dir(examples)
        .expect("examples exist")
        .filter_map(Result::ok)
    {
        let path = entry.path();
        if path
            .extension()
            .is_some_and(|extension| extension == "q0lang")
        {
            checked += 1;
            let source = std::fs::read_to_string(&path).expect("readable example");
            let compiled = q0s_format::q0lang::compiler::compile_source(&source);
            if !compiled.diagnostics.is_empty() {
                failures.push(format!("{}: {:?}", path.display(), compiled.diagnostics));
            }
        }
    }
    assert!(checked > 20, "only {checked} examples found");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn the_quick_start_prints_what_the_readme_says() {
    let source = std::fs::read_to_string(docs_dir().join("examples").join("hello.q0lang"))
        .expect("quick start example");
    let mut runtime = q0s_format::q0lang::runtime::Runtime::new();
    let report = runtime.execute_source(&source);
    assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
    let printed = report
        .actions
        .iter()
        .filter_map(|action| match action {
            q0s_format::q0lang::runtime::RuntimeAction::ShellCommand { command, args }
                if command == "print" =>
            {
                Some(
                    args.iter()
                        .map(|value| value.display_lossy())
                        .collect::<Vec<_>>()
                        .join(" "),
                )
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        printed,
        [
            "step 0 x = 0",
            "step 1 x = 57.8125",
            "step 2 x = 87.5",
            "step 3 x = 98.4375",
            "step 4 x = 100"
        ]
    );
}
