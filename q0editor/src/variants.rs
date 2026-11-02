//! Variants: copies of a symbol kept together as one family, like a pan and
//! its broken and dented versions. A variant is named after the symbol with
//! a `~` and a tag ("Pan ~2", which can be renamed "Pan ~broken"), and an
//! instance can be flipped through its family from the property panel
//! without reaching for the library.

use q0s_format::v2::ProjectV2;

/// What separates a symbol's name from its variant tag.
const MARK: &str = " ~";

/// The family name a symbol's name belongs to: everything before ` ~`.
pub fn base_name(name: &str) -> &str {
    name.split_once(MARK)
        .map_or(name, |(base, _)| base)
        .trim_end()
}

/// The symbols in `q0rg_id`'s family, the plain one first and the rest by
/// name.
pub fn family(project: &ProjectV2, q0rg_id: u16) -> Vec<u16> {
    let Some(symbol) = project.q0rgs.iter().find(|q0rg| q0rg.q0rg_id == q0rg_id) else {
        return Vec::new();
    };
    let base = base_name(&symbol.name);
    let mut members: Vec<(bool, &str, u16)> = project
        .q0rgs
        .iter()
        .filter(|q0rg| base_name(&q0rg.name) == base)
        .map(|q0rg| (q0rg.name.contains(MARK), q0rg.name.as_str(), q0rg.q0rg_id))
        .collect();
    members.sort();
    members.into_iter().map(|(_, _, id)| id).collect()
}

/// A name for a new variant of `name`'s family: `base ~2`, `base ~3`, ...
pub fn new_variant_name(project: &ProjectV2, name: &str) -> String {
    let base = base_name(name);
    (2..)
        .map(|number| format!("{base}{MARK}{number}"))
        .find(|candidate| project.q0rgs.iter().all(|q0rg| q0rg.name != *candidate))
        .expect("an unused number")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variants_gather_under_the_name_before_the_tilde() {
        assert_eq!(base_name("Pan"), "Pan");
        assert_eq!(base_name("Pan ~broken"), "Pan");
        assert_eq!(base_name("Pan ~2"), "Pan");

        let mut project = crate::app::EditorApp::default().state.project;
        let template = project.q0rgs[0].clone();
        for (id, name) in [
            (50, "Pan ~broken"),
            (51, "Pan"),
            (52, "Pot"),
            (53, "Pan ~2"),
        ] {
            let mut symbol = template.clone();
            symbol.q0rg_id = id;
            symbol.name = name.to_string();
            project.q0rgs.push(symbol);
        }
        assert_eq!(family(&project, 50), vec![51, 53, 50]);
        assert_eq!(family(&project, 52), vec![52]);
        assert_eq!(new_variant_name(&project, "Pan ~broken"), "Pan ~3");
        assert_eq!(new_variant_name(&project, "Pot"), "Pot ~2");
    }
}
