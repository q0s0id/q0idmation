//! A plugin file as the plugin list and the studio see it: its card, page,
//! settings and pictures, read from and written back into its program.

use base64::Engine;

use crate::ast::*;
use crate::parser::{parse, ParseError};
use crate::printer::print;

/// The plugin's card.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Meta {
    pub name: String,
    /// Unique, like `yourname.pluginname`; plugin files are stored by it.
    pub id: String,
    pub version: String,
    pub author: String,
    /// One line under the name in the list.
    pub summary: String,
    /// The card's accent colour.
    pub color: String,
    /// The blob shown as the plugin's picture.
    pub thumbnail: String,
}

/// A file carried inside the plugin, base64 in the text.
#[derive(Debug, Clone, PartialEq)]
pub struct Blob {
    pub name: String,
    /// What it is: `png`, `jpg`, `txt`...
    pub kind: String,
    pub data: String,
}

impl Blob {
    pub fn new(name: &str, kind: &str, bytes: &[u8]) -> Self {
        Self {
            name: name.into(),
            kind: kind.into(),
            data: base64::engine::general_purpose::STANDARD.encode(bytes),
        }
    }

    pub fn bytes(&self) -> Option<Vec<u8>> {
        base64::engine::general_purpose::STANDARD
            .decode(self.data.as_bytes())
            .ok()
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PluginDoc {
    pub program: Program,
}

/// Turns a name into an id piece: lower case, dashes for the rest.
pub fn slug(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() {
        "plugin".into()
    } else {
        out
    }
}

fn literal_text(expr: &Expr) -> String {
    match &expr.kind {
        ExprKind::Str(text) => text.clone(),
        ExprKind::Num(value, _) => crate::printer::number(*value),
        ExprKind::Bool(value) => value.to_string(),
        _ => String::new(),
    }
}

impl PluginDoc {
    pub fn parse(src: &str) -> Result<Self, ParseError> {
        Ok(Self {
            program: parse(src)?,
        })
    }

    pub fn source(&self) -> String {
        print(&self.program)
    }

    pub fn meta(&self) -> Meta {
        let mut meta = Meta::default();
        for item in &self.program.items {
            if let ItemKind::Plugin { name, fields } = &item.kind {
                meta.name = name.clone();
                for (key, value) in fields {
                    let text = literal_text(value);
                    match key.as_str() {
                        "id" => meta.id = text,
                        "version" => meta.version = text,
                        "author" => meta.author = text,
                        "summary" => meta.summary = text,
                        "color" => meta.color = text,
                        "thumbnail" => meta.thumbnail = text,
                        _ => {}
                    }
                }
                break;
            }
        }
        if meta.name.is_empty() {
            meta.name = "Untitled plugin".into();
        }
        if meta.id.is_empty() {
            meta.id = slug(&meta.name);
        }
        meta
    }

    pub fn set_meta(&mut self, meta: &Meta) {
        let mut fields = Vec::new();
        let mut push = |key: &str, value: &str| {
            if !value.is_empty() {
                fields.push((key.to_string(), Expr::new(ExprKind::Str(value.into()), 0)));
            }
        };
        push("id", &meta.id);
        push("version", &meta.version);
        push("author", &meta.author);
        push("summary", &meta.summary);
        push("color", &meta.color);
        push("thumbnail", &meta.thumbnail);
        // Fields this form does not know stay as they were.
        if let Some(ItemKind::Plugin { fields: old, .. }) = self
            .program
            .items
            .iter()
            .map(|item| &item.kind)
            .find(|kind| matches!(kind, ItemKind::Plugin { .. }))
        {
            for (key, value) in old {
                if !fields.iter().any(|(known, _)| known == key)
                    && !["id", "version", "author", "summary", "color", "thumbnail"]
                        .contains(&key.as_str())
                {
                    fields.push((key.clone(), value.clone()));
                }
            }
        }
        let kind = ItemKind::Plugin {
            name: meta.name.clone(),
            fields,
        };
        self.put_item(|kind| matches!(kind, ItemKind::Plugin { .. }), kind, 0);
    }

    /// Replaces the first item `is` matches, or puts `kind` at `index`.
    fn put_item(&mut self, is: impl Fn(&ItemKind) -> bool, kind: ItemKind, index: usize) {
        if let Some(item) = self.program.items.iter_mut().find(|item| is(&item.kind)) {
            item.kind = kind;
            return;
        }
        let index = index.min(self.program.items.len());
        self.program.items.insert(
            index,
            Item {
                kind,
                line: 0,
                pos: None,
            },
        );
    }

    /// Where a section without its own place goes: after the card.
    fn after_plugin(&self) -> usize {
        self.program
            .items
            .iter()
            .position(|item| {
                !matches!(
                    item.kind,
                    ItemKind::Plugin { .. }
                        | ItemKind::Page(_)
                        | ItemKind::Settings(_)
                        | ItemKind::Stmt(Stmt {
                            kind: StmtKind::Comment(_),
                            ..
                        })
                )
            })
            .unwrap_or(self.program.items.len())
    }

    pub fn page(&self) -> String {
        self.program
            .items
            .iter()
            .find_map(|item| match &item.kind {
                ItemKind::Page(text) => Some(text.clone()),
                _ => None,
            })
            .unwrap_or_default()
    }

    pub fn set_page(&mut self, text: &str) {
        if text.trim().is_empty() {
            self.program
                .items
                .retain(|item| !matches!(item.kind, ItemKind::Page(_)));
            return;
        }
        let index = self.after_plugin();
        self.put_item(
            |kind| matches!(kind, ItemKind::Page(_)),
            ItemKind::Page(text.to_string()),
            index,
        );
    }

    pub fn settings(&self) -> Vec<SettingDecl> {
        self.program
            .items
            .iter()
            .find_map(|item| match &item.kind {
                ItemKind::Settings(decls) => Some(decls.clone()),
                _ => None,
            })
            .unwrap_or_default()
    }

    pub fn set_settings(&mut self, decls: Vec<SettingDecl>) {
        if decls.is_empty() {
            self.program
                .items
                .retain(|item| !matches!(item.kind, ItemKind::Settings(_)));
            return;
        }
        let index = self.after_plugin();
        self.put_item(
            |kind| matches!(kind, ItemKind::Settings(_)),
            ItemKind::Settings(decls),
            index,
        );
    }

    pub fn blobs(&self) -> Vec<Blob> {
        self.program
            .items
            .iter()
            .filter_map(|item| match &item.kind {
                ItemKind::Blob { name, kind, data } => Some(Blob {
                    name: name.clone(),
                    kind: kind.clone(),
                    data: data.clone(),
                }),
                _ => None,
            })
            .collect()
    }

    pub fn blob(&self, name: &str) -> Option<Blob> {
        self.blobs().into_iter().find(|blob| blob.name == name)
    }

    pub fn set_blob(&mut self, blob: Blob) {
        let name = blob.name.clone();
        let kind = ItemKind::Blob {
            name: blob.name,
            kind: blob.kind,
            data: blob.data,
        };
        let end = self.program.items.len();
        self.put_item(
            |kind| matches!(kind, ItemKind::Blob { name: other, .. } if *other == name),
            kind,
            end,
        );
    }

    pub fn remove_blob(&mut self, name: &str) {
        self.program.items.retain(
            |item| !matches!(&item.kind, ItemKind::Blob { name: other, .. } if other == name),
        );
    }

    /// The thumbnail's bytes, if the plugin has one.
    pub fn thumbnail(&self) -> Option<Vec<u8>> {
        let meta = self.meta();
        let name = if meta.thumbnail.is_empty() {
            "thumbnail"
        } else {
            &meta.thumbnail
        };
        self.blob(name)?.bytes()
    }
}

/// A new plugin to start from.
pub fn template(name: &str, author: &str) -> String {
    let id = format!("{}.{}", slug(author), slug(name));
    format!(
        r##"# {name}, a q0editor plugin written in q0ex.
plugin {name:?} {{
  id = {id:?}
  version = "1.0.0"
  author = {author:?}
  summary = "What it does, in one line."
  color = "#9d4eff"
}}

page ~~~
# {name}
Tell people what your plugin does. **Bold**, *italic*, {{#9d4eff}}colours{{/}}
and pictures from the plugin all work here.

- one thing it does
- another thing
~~~

settings {{
  slider amount "Amount" = 5 from 1 to 20 ? "How much of it to do"
  toggle loud "Say hello on start" = true
}}

keep runs = 0

when enabled {{
  runs += 1
  if config.loud {{
    say! `{name} is on. Started {{runs}} times so far!`
  }}
}}

when menu "Do the thing" {{
  for obj in project.selection() {{
    obj.rotation += config.amount
  }}
  say! "done!"
}}
"##
    )
}

/// The text without its blob sections, and the blobs, for an editor that
/// should not show pages of base64.
pub fn split_blobs(src: &str) -> (String, Vec<Blob>) {
    let mut text = String::new();
    let mut blobs = Vec::new();
    let mut lines = src.lines().peekable();
    while let Some(line) = lines.next() {
        let trimmed = line.trim();
        let header = trimmed
            .strip_suffix("~~~")
            .map(str::trim_end)
            .and_then(|head| head.strip_prefix("blob "))
            .map(|rest| rest.split_whitespace().collect::<Vec<_>>());
        if let Some([name, kind]) = header.as_deref() {
            let mut data = String::new();
            let mut closed = false;
            for inner in lines.by_ref() {
                if inner.trim() == "~~~" {
                    closed = true;
                    break;
                }
                data.push_str(inner.trim());
            }
            if closed {
                blobs.push(Blob {
                    name: name.to_string(),
                    kind: kind.to_string(),
                    data,
                });
                // Drop the blank line that separated it, if any.
                while text.ends_with("\n\n") {
                    text.pop();
                }
                continue;
            }
            text.push_str(line);
            text.push('\n');
            text.push_str(&data);
            continue;
        }
        text.push_str(line);
        text.push('\n');
    }
    (text, blobs)
}

/// `text` with `blobs` written at its end.
pub fn join_blobs(text: &str, blobs: &[Blob]) -> String {
    let mut out = text.trim_end().to_string();
    out.push('\n');
    for blob in blobs {
        out.push('\n');
        out.push_str(&format!("blob {} {} ~~~\n", blob.name, blob.kind));
        for chunk in blob.data.as_bytes().chunks(76) {
            out.push_str(std::str::from_utf8(chunk).unwrap_or_default());
            out.push('\n');
        }
        out.push_str("~~~\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_template_parses_and_knows_itself() {
        let doc = PluginDoc::parse(&template("Spin It", "q0s")).unwrap();
        let meta = doc.meta();
        assert_eq!(meta.name, "Spin It");
        assert_eq!(meta.id, "q0s.spin-it");
        assert_eq!(doc.settings().len(), 2);
        assert!(doc.page().contains("# Spin It"));
    }

    #[test]
    fn editing_the_card_page_settings_and_blobs_round_trips() {
        let mut doc = PluginDoc::parse(&template("Spin", "me")).unwrap();
        let mut meta = doc.meta();
        meta.summary = "Spins \"things\".".into();
        meta.thumbnail = "thumb".into();
        doc.set_meta(&meta);
        doc.set_page("# New page\n{red}hi{/}");
        let mut settings = doc.settings();
        settings.truncate(1);
        doc.set_settings(settings);
        doc.set_blob(Blob::new("thumb", "png", &[1, 2, 3, 250]));
        let again = PluginDoc::parse(&doc.source()).unwrap();
        assert_eq!(again.meta(), meta);
        assert_eq!(again.page(), "# New page\n{red}hi{/}");
        assert_eq!(again.settings().len(), 1);
        assert_eq!(again.thumbnail(), Some(vec![1, 2, 3, 250]));
    }

    #[test]
    fn blobs_split_out_and_join_back() {
        let mut doc = PluginDoc::parse(&template("Spin", "me")).unwrap();
        doc.set_blob(Blob::new("thumbnail", "png", &[9; 200]));
        let source = doc.source();
        let (text, blobs) = split_blobs(&source);
        assert!(!text.contains("blob thumbnail"));
        assert_eq!(blobs.len(), 1);
        let joined = join_blobs(&text, &blobs);
        let again = PluginDoc::parse(&joined).unwrap();
        assert_eq!(again.thumbnail(), Some(vec![9; 200]));
    }

    #[test]
    fn slugs_are_tidy() {
        assert_eq!(slug("  My Cool Plugin!! "), "my-cool-plugin");
        assert_eq!(slug("???"), "plugin");
    }
}
