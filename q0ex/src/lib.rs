//! q0ex: the language q0editor plugins are written in.
//!
//! A plugin is one `.qx` (or `.q0ex`) text file. It holds the plugin's
//! card (`plugin { }`), its page (`page ~~~ ... ~~~`), its settings
//! (`settings { }`), its pictures (`blob ... ~~~ ... ~~~`) and its code:
//! `when` handlers, functions and plain statements.
//!
//! This crate knows nothing about the editor. [`interp::Host`] is how the
//! editor lends the language its `editor`, `project`, `ui` and other
//! namespaces.

pub mod ast;
pub mod doc;
pub mod interp;
pub mod lexer;
pub mod page;
pub mod parser;
pub mod printer;
pub mod stdlib;
pub mod value;

pub use ast::{Expr, ExprKind, Item, ItemKind, Program, SettingDecl, SettingKind, Stmt, StmtKind};
pub use doc::{Blob, Meta, PluginDoc};
pub use interp::{Handle, Host, Interp, RunError};
pub use parser::{parse, ParseError};
pub use value::Value;

/// The file extensions of a plugin; both mean the same.
pub const EXTENSIONS: [&str; 2] = ["qx", "q0ex"];

/// Whether `path` names a plugin file.
pub fn is_plugin_path(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            EXTENSIONS
                .iter()
                .any(|known| extension.eq_ignore_ascii_case(known))
        })
}
