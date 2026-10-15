mod binary_fla;
pub mod fonts;
mod text_outline;
mod xfl_content;

use std::fmt;
use std::fs::File;
use std::io::{Cursor, Read, Seek};
use std::path::Path;

use q0s_format::v2::{
    Layer, LayerKey, LayerKind, LayerMetadata, ProjectMeta, ProjectV2, Q0rg, Rgba,
};
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;

pub const IMPORT_EXTENSIONS: &[&str] = &["fla", "xfl"];
const MAX_FLA_BYTES: u64 = 512 * 1024 * 1024;
const MAX_ARCHIVE_ENTRIES: usize = 20_000;
const MAX_ARCHIVE_UNCOMPRESSED_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_DOM_DOCUMENT_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug)]
pub enum ImportError {
    Io(std::io::Error),
    Archive(zip::result::ZipError),
    Xml(String),
    Unsupported(String),
    Invalid(String),
}

impl fmt::Display for ImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "io error: {error}"),
            Self::Archive(error) => write!(f, "fla archive error: {error}"),
            Self::Xml(error) => write!(f, "xfl xml error: {error}"),
            Self::Unsupported(error) => write!(f, "unsupported fla/xfl: {error}"),
            Self::Invalid(error) => write!(f, "invalid fla/xfl: {error}"),
        }
    }
}

impl std::error::Error for ImportError {}

impl From<std::io::Error> for ImportError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<zip::result::ZipError> for ImportError {
    fn from(value: zip::result::ZipError) -> Self {
        Self::Archive(value)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportReport {
    pub source_kind: String,
    pub imported_layers: usize,
    pub imported_keyframes: usize,
    pub warnings: Vec<String>,
}

impl ImportReport {
    pub fn summary(&self) -> String {
        let warning_suffix = if self.warnings.is_empty() {
            String::new()
        } else {
            format!(", {} warning(s)", self.warnings.len())
        };
        format!(
            "{}: {} layer(s), {} keyframe(s){}",
            self.source_kind, self.imported_layers, self.imported_keyframes, warning_suffix
        )
    }
}

#[derive(Debug, Clone)]
pub struct ImportedProject {
    pub project: ProjectV2,
    pub report: ImportReport,
    pub(crate) pending_text: Vec<PendingText>,
}

#[derive(Debug, Clone)]
pub(crate) struct PendingText {
    asset_id: u16,
    source: xfl_content::RawText,
    replacement: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingFont {
    pub name: String,
    pub replacement: String,
    pub text_count: usize,
}

impl ImportedProject {
    pub fn missing_fonts(&self) -> Vec<MissingFont> {
        let mut rows = std::collections::BTreeMap::<String, MissingFont>::new();
        for text in &self.pending_text {
            rows.entry(text.source.font.to_lowercase()).or_insert_with(|| MissingFont {
                name: text.source.font.clone(), replacement: text.replacement.clone(), text_count: 0,
            }).text_count += 1;
        }
        rows.into_values().collect()
    }

    /// Validate and render every replacement before changing the imported
    /// project. A bad choice leaves all text and pending choices intact.
    pub fn replace_missing_fonts(&mut self, choices: &std::collections::BTreeMap<String, String>) -> Result<(), String> {
        let mut replacements = Vec::with_capacity(self.pending_text.len());
        for pending in &self.pending_text {
            let name = choices.iter().find(|(name, _)| name.eq_ignore_ascii_case(&pending.source.font))
                .map(|(_, replacement)| replacement).ok_or_else(|| format!("Choose a replacement for '{}'", pending.source.font))?;
            let mut source = pending.source.clone();
            source.font = name.clone();
            let paths = xfl_content::text_paths(&source)?;
            let index = self.project.assets.iter().position(|asset| matches!(asset, q0s_format::v2::Asset::Vector(vector) if vector.asset_id == pending.asset_id))
                .ok_or("Imported text asset is missing")?;
            replacements.push((index, paths));
        }
        for (index, paths) in replacements {
            if let q0s_format::v2::Asset::Vector(vector) = &mut self.project.assets[index] {
                vector.paths = paths;
            }
        }
        self.pending_text.clear();
        Ok(())
    }
}

pub(crate) fn normalize_folder_layer_order(
    q0rg: &mut Q0rg,
    layer_metadata: &std::collections::HashMap<LayerKey, LayerMetadata>,
) {
    use std::collections::{HashMap, HashSet};

    let folder_ids: HashSet<u16> = q0rg
        .layers
        .iter()
        .filter(|layer| {
            layer_metadata
                .get(&LayerKey::new(q0rg.q0rg_id, layer.layer_id))
                .is_some_and(|metadata| {
                    matches!(metadata.kind, LayerKind::Folder | LayerKind::Mask)
                })
        })
        .map(|layer| layer.layer_id)
        .collect();
    let mut children = HashMap::<u16, Vec<u16>>::new();
    let mut roots = Vec::new();
    for layer in &q0rg.layers {
        let parent = layer_metadata
            .get(&LayerKey::new(q0rg.q0rg_id, layer.layer_id))
            .and_then(|metadata| metadata.parent_folder_id.or(metadata.mask_layer_id))
            .filter(|parent| folder_ids.contains(parent));
        if let Some(parent) = parent {
            children.entry(parent).or_default().push(layer.layer_id);
        } else {
            roots.push(layer.layer_id);
        }
    }

    fn append_subtree(
        root: u16,
        children: &HashMap<u16, Vec<u16>>,
        scheduled: &mut HashSet<u16>,
        order: &mut Vec<u16>,
    ) {
        let mut pending = vec![(root, false)];
        while let Some((layer_id, expanded)) = pending.pop() {
            if expanded {
                order.push(layer_id);
                continue;
            }
            if !scheduled.insert(layer_id) {
                continue;
            }
            pending.push((layer_id, true));
            if let Some(descendants) = children.get(&layer_id) {
                for child in descendants.iter().rev() {
                    if !scheduled.contains(child) {
                        pending.push((*child, false));
                    }
                }
            }
        }
    }

    let mut scheduled = HashSet::new();
    let mut order = Vec::with_capacity(q0rg.layers.len());
    for root in roots {
        append_subtree(root, &children, &mut scheduled, &mut order);
    }
    // Preserve malformed cyclic data for the validator to reject cleanly,
    // without recursing on an untrusted XFL folder graph.
    for layer in &q0rg.layers {
        if !scheduled.contains(&layer.layer_id) {
            append_subtree(layer.layer_id, &children, &mut scheduled, &mut order);
        }
    }
    if order.len() != q0rg.layers.len() {
        return;
    }

    let mut by_id: HashMap<u16, Layer> = std::mem::take(&mut q0rg.layers)
        .into_iter()
        .map(|layer| (layer.layer_id, layer))
        .collect();
    q0rg.layers = order
        .into_iter()
        .filter_map(|layer_id| by_id.remove(&layer_id))
        .collect();
}

#[derive(Debug, Clone)]
struct ImportedLayer {
    name: String,
    keyframes: Vec<u16>,
    frame_end: u64,
    kind: LayerKind,
    masked: bool,
    mask_source_index: Option<usize>,
    parent_index: Option<usize>,
    hidden: bool,
    locked: bool,
}

impl ImportedLayer {
    fn from_element(
        reader: &Reader<&[u8]>,
        element: &BytesStart<'_>,
        ordinal: usize,
        warnings: &mut Vec<String>,
    ) -> Result<Self, ImportError> {
        let name = attr(reader, element, b"name")?
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| format!("Layer {}", ordinal + 1));
        let layer_type = attr(reader, element, b"layerType")?.unwrap_or_default();
        let kind = match layer_type.to_ascii_lowercase().as_str() {
            "" | "normal" => LayerKind::Normal,
            "folder" => LayerKind::Folder,
            "mask" => LayerKind::Mask,
            "masked" => LayerKind::Normal,
            unsupported => {
                warnings.push(format!(
                    "layer '{name}' uses Animate layerType='{unsupported}'; imported as a normal layer"
                ));
                LayerKind::Normal
            }
        };
        let parent_index = attr(reader, element, b"parentLayerIndex")?
            .map(|value| {
                value.parse::<usize>().map_err(|_| {
                    ImportError::Invalid(format!(
                        "layer '{name}' has invalid parentLayerIndex '{value}'"
                    ))
                })
            })
            .transpose()?;
        let hidden = matches!(
            attr(reader, element, b"visible")?.as_deref(),
            Some("false") | Some("0")
        );
        let locked = matches!(
            attr(reader, element, b"locked")?.as_deref(),
            Some("true") | Some("1")
        );
        Ok(Self {
            name,
            keyframes: Vec::new(),
            frame_end: 0,
            kind,
            masked: layer_type.eq_ignore_ascii_case("masked"),
            mask_source_index: None,
            parent_index,
            hidden,
            locked,
        })
    }
}

pub fn is_import_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            IMPORT_EXTENSIONS
                .iter()
                .any(|candidate| extension.eq_ignore_ascii_case(candidate))
        })
}

pub fn import_path(path: &Path) -> Result<ImportedProject, ImportError> {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default();
    if extension.eq_ignore_ascii_case("fla") {
        import_fla(path)
    } else if extension.eq_ignore_ascii_case("xfl") {
        import_xfl(path)
    } else {
        Err(ImportError::Unsupported(format!(
            "expected .fla or .xfl, got {}",
            path.display()
        )))
    }
}

pub fn import_fla(path: &Path) -> Result<ImportedProject, ImportError> {
    let size = std::fs::metadata(path)?.len();
    if size > MAX_FLA_BYTES {
        return Err(ImportError::Invalid(format!(
            "archive is too large ({size} bytes; limit is {MAX_FLA_BYTES})"
        )));
    }

    let mut file = File::open(path)?;
    let mut signature = [0_u8; 8];
    let signature_len = file.read(&mut signature)?;
    if signature_len == signature.len() && signature == binary_fla::OLE2_MAGIC {
        return binary_fla::import_legacy_fla(path);
    }

    let file = File::open(path)?;
    match zip::ZipArchive::new(file) {
        Ok(archive) => import_fla_archive(path, archive, "fla"),
        Err(zip::result::ZipError::InvalidArchive(_)) => {
            let mut bytes = std::fs::read(path)?;
            if repair_adobe_central_directory_size(&mut bytes) {
                let archive = zip::ZipArchive::new(Cursor::new(bytes))?;
                import_fla_archive(path, archive, "fla (repaired adobe zip directory)")
            } else {
                Err(ImportError::Unsupported(
                    "this is neither a readable XFL-based FLA archive nor an ole2 legacy binary FLA"
                        .to_string(),
                ))
            }
        }
        Err(other) => Err(ImportError::Archive(other)),
    }
}

fn import_fla_archive<R: Read + Seek>(
    path: &Path,
    mut archive: zip::ZipArchive<R>,
    source_kind: &str,
) -> Result<ImportedProject, ImportError> {
    if archive.len() > MAX_ARCHIVE_ENTRIES {
        return Err(ImportError::Invalid(format!(
            "archive contains too many entries ({}; limit is {MAX_ARCHIVE_ENTRIES})",
            archive.len()
        )));
    }

    let mut total_uncompressed = 0_u64;
    let mut dom_index = None;
    let mut library_indices = Vec::new();
    let mut binary_indices = Vec::new();
    for index in 0..archive.len() {
        let entry = archive.by_index(index)?;
        if entry.enclosed_name().is_none() {
            return Err(ImportError::Invalid(format!(
                "archive entry has an unsafe path: {}",
                entry.name()
            )));
        }
        total_uncompressed = total_uncompressed
            .checked_add(entry.size())
            .ok_or_else(|| ImportError::Invalid("archive size overflow".to_string()))?;
        if total_uncompressed > MAX_ARCHIVE_UNCOMPRESSED_BYTES {
            return Err(ImportError::Invalid(format!(
                "archive expands past the {} byte safety limit",
                MAX_ARCHIVE_UNCOMPRESSED_BYTES
            )));
        }
        let normalized = normalize_archive_name(entry.name());
        if normalized.eq_ignore_ascii_case("DOMDocument.xml") {
            dom_index = Some(index);
        } else {
            let lower = normalized.to_ascii_lowercase();
            if lower.starts_with("library/") && lower.ends_with(".xml") {
                library_indices.push(index);
            } else if lower.starts_with("bin/") && lower.ends_with(".dat") {
                binary_indices.push(index);
            }
        }
    }

    let dom_index = dom_index.ok_or_else(|| {
        ImportError::Invalid("FLA archive does not contain DOMDocument.xml".to_string())
    })?;
    let mut dom = archive.by_index(dom_index)?;
    if dom.size() > MAX_DOM_DOCUMENT_BYTES {
        return Err(ImportError::Invalid(format!(
            "DOMDocument.xml is too large ({} bytes; limit is {MAX_DOM_DOCUMENT_BYTES})",
            dom.size()
        )));
    }
    let mut xml = Vec::with_capacity(dom.size() as usize);
    dom.read_to_end(&mut xml)?;
    drop(dom);

    let mut library_docs = Vec::with_capacity(library_indices.len());
    for index in library_indices {
        let mut entry = archive.by_index(index)?;
        if entry.size() > MAX_DOM_DOCUMENT_BYTES {
            return Err(ImportError::Invalid(format!(
                "library xml '{}' is too large ({} bytes)",
                entry.name(),
                entry.size()
            )));
        }
        let name = normalize_archive_name(entry.name());
        let mut bytes = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut bytes)?;
        library_docs.push((name, bytes));
    }

    let mut binary_docs = Vec::with_capacity(binary_indices.len());
    for index in binary_indices {
        let mut entry = archive.by_index(index)?;
        let name = normalize_archive_name(entry.name());
        let mut bytes = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut bytes)?;
        binary_docs.push((name, bytes));
    }

    let mut imported = import_dom_document(path, &xml, source_kind)?;
    xfl_content::enrich_project(&mut imported, &xml, &library_docs, &binary_docs)?;
    Ok(imported)
}

/// Animate sometimes writes an otherwise valid XFL-based FLA whose EOCD
/// central-directory size is larger than the bytes actually present before
/// the EOCD record. Strict ZIP readers reject that archive even though the
/// directory offset and every directory entry are valid. Repair only this
/// narrowly recognizable mismatch in-memory; the source FLA is never changed.
fn repair_adobe_central_directory_size(bytes: &mut [u8]) -> bool {
    const EOCD_LEN: usize = 22;
    const EOCD_SIGNATURE: &[u8; 4] = b"PK\x05\x06";
    const CENTRAL_SIGNATURE: &[u8; 4] = b"PK\x01\x02";

    if bytes.len() < EOCD_LEN {
        return false;
    }
    let search_start = bytes.len().saturating_sub(EOCD_LEN + u16::MAX as usize);
    let Some(eocd) = bytes[search_start..]
        .windows(EOCD_SIGNATURE.len())
        .rposition(|window| window == EOCD_SIGNATURE)
        .map(|offset| search_start + offset)
    else {
        return false;
    };
    if eocd + EOCD_LEN > bytes.len() {
        return false;
    }

    let disk = u16::from_le_bytes([bytes[eocd + 4], bytes[eocd + 5]]);
    let central_disk = u16::from_le_bytes([bytes[eocd + 6], bytes[eocd + 7]]);
    let comment_len = u16::from_le_bytes([bytes[eocd + 20], bytes[eocd + 21]]) as usize;
    if disk != 0 || central_disk != 0 || eocd + EOCD_LEN + comment_len != bytes.len() {
        return false;
    }

    let central_offset = u32::from_le_bytes([
        bytes[eocd + 16],
        bytes[eocd + 17],
        bytes[eocd + 18],
        bytes[eocd + 19],
    ]) as usize;
    if central_offset >= eocd
        || central_offset + CENTRAL_SIGNATURE.len() > eocd
        || &bytes[central_offset..central_offset + CENTRAL_SIGNATURE.len()] != CENTRAL_SIGNATURE
    {
        return false;
    }

    let declared_size = u32::from_le_bytes([
        bytes[eocd + 12],
        bytes[eocd + 13],
        bytes[eocd + 14],
        bytes[eocd + 15],
    ]) as usize;
    let actual_size = eocd - central_offset;
    if declared_size <= actual_size || actual_size > u32::MAX as usize {
        return false;
    }

    bytes[eocd + 12..eocd + 16].copy_from_slice(&(actual_size as u32).to_le_bytes());
    true
}

pub fn import_xfl(path: &Path) -> Result<ImportedProject, ImportError> {
    let dom_path = if path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("DOMDocument.xml"))
    {
        path.to_path_buf()
    } else {
        path.parent()
            .unwrap_or_else(|| Path::new("."))
            .join("DOMDocument.xml")
    };
    let size = std::fs::metadata(&dom_path)?.len();
    if size > MAX_DOM_DOCUMENT_BYTES {
        return Err(ImportError::Invalid(format!(
            "DOMDocument.xml is too large ({size} bytes; limit is {MAX_DOM_DOCUMENT_BYTES})"
        )));
    }
    let xml = std::fs::read(&dom_path)?;
    let mut imported = import_dom_document(path, &xml, "xfl")?;
    let mut library_docs = Vec::new();
    if let Some(root) = dom_path.parent() {
        collect_library_xml(&root.join("LIBRARY"), root, &mut library_docs)?;
    }
    let mut binary_docs = Vec::new();
    if let Some(root) = dom_path.parent() {
        collect_binary_dat(&root.join("bin"), root, &mut binary_docs)?;
    }
    xfl_content::enrich_project(&mut imported, &xml, &library_docs, &binary_docs)?;
    Ok(imported)
}

fn collect_library_xml(
    directory: &Path,
    root: &Path,
    output: &mut Vec<(String, Vec<u8>)>,
) -> Result<(), ImportError> {
    if !directory.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_library_xml(&path, root, output)?;
        } else if path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("xml"))
        {
            let size = entry.metadata()?.len();
            if size > MAX_DOM_DOCUMENT_BYTES {
                return Err(ImportError::Invalid(format!(
                    "library xml '{}' is too large ({size} bytes)",
                    path.display()
                )));
            }
            let relative = path.strip_prefix(root).unwrap_or(&path);
            output.push((
                relative.to_string_lossy().replace('\\', "/"),
                std::fs::read(&path)?,
            ));
        }
    }
    Ok(())
}

fn collect_binary_dat(
    directory: &Path,
    root: &Path,
    output: &mut Vec<(String, Vec<u8>)>,
) -> Result<(), ImportError> {
    if !directory.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file()
            && path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("dat"))
        {
            let relative = path.strip_prefix(root).unwrap_or(&path);
            output.push((
                relative.to_string_lossy().replace('\\', "/"),
                std::fs::read(&path)?,
            ));
        }
    }
    Ok(())
}
fn import_dom_document(
    source_path: &Path,
    xml: &[u8],
    source_kind: &str,
) -> Result<ImportedProject, ImportError> {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut width = None::<f64>;
    let mut height = None::<f64>;
    let mut frame_rate = None::<f64>;
    let mut background_color = None::<Rgba>;
    let mut layers = Vec::<ImportedLayer>::new();
    let mut current_layer = None::<ImportedLayer>;
    let mut current_layer_depth = None::<usize>;
    let mut timeline_depth = None::<usize>;
    let mut saw_first_timeline = false;
    let mut depth = 0_usize;
    let mut max_frame_end = 0_u64;
    let mut warnings = Vec::new();

    loop {
        match reader
            .read_event_into(&mut buf)
            .map_err(|error| ImportError::Xml(error.to_string()))?
        {
            Event::Start(element) => {
                depth += 1;
                let name = element.name();
                let name = name.as_ref();
                if name == b"DOMDocument" {
                    width = parse_optional_f64(&reader, &element, b"width")?;
                    height = parse_optional_f64(&reader, &element, b"height")?;
                    frame_rate = parse_optional_f64(&reader, &element, b"frameRate")?;
                    background_color = parse_optional_color(&reader, &element, b"backgroundColor")?;
                } else if name == b"DOMTimeline" && !saw_first_timeline {
                    saw_first_timeline = true;
                    timeline_depth = Some(depth);
                } else if timeline_depth.is_some() && name == b"DOMLayer" && current_layer.is_none()
                {
                    current_layer = Some(ImportedLayer::from_element(
                        &reader,
                        &element,
                        layers.len(),
                        &mut warnings,
                    )?);
                    current_layer_depth = Some(depth);
                } else if name == b"DOMFrame" {
                    if let Some(layer) = current_layer.as_mut() {
                        import_frame(&reader, &element, layer, &mut max_frame_end)?;
                    }
                }
            }
            Event::Empty(element) => {
                let name = element.name();
                let name = name.as_ref();
                if name == b"DOMDocument" {
                    width = parse_optional_f64(&reader, &element, b"width")?;
                    height = parse_optional_f64(&reader, &element, b"height")?;
                    frame_rate = parse_optional_f64(&reader, &element, b"frameRate")?;
                    background_color = parse_optional_color(&reader, &element, b"backgroundColor")?;
                } else if timeline_depth.is_some() && name == b"DOMLayer" && current_layer.is_none()
                {
                    layers.push(ImportedLayer::from_element(
                        &reader,
                        &element,
                        layers.len(),
                        &mut warnings,
                    )?);
                } else if name == b"DOMFrame" {
                    if let Some(layer) = current_layer.as_mut() {
                        import_frame(&reader, &element, layer, &mut max_frame_end)?;
                    }
                }
            }
            Event::End(element) => {
                let name = element.name();
                let name = name.as_ref();
                if name == b"DOMLayer" && current_layer_depth == Some(depth) {
                    layers.push(current_layer.take().expect("layer depth has a layer"));
                    current_layer_depth = None;
                }
                if name == b"DOMTimeline" && timeline_depth == Some(depth) {
                    timeline_depth = None;
                }
                depth = depth.saturating_sub(1);
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    if width.is_none() && height.is_none() && frame_rate.is_none() && !saw_first_timeline {
        return Err(ImportError::Invalid(
            "DOMDocument.xml does not look like an Animate XFL document".to_string(),
        ));
    }

    let width = convert_dimension(width.unwrap_or(640.0), "width", &mut warnings)?;
    let height = convert_dimension(height.unwrap_or(480.0), "height", &mut warnings)?;
    let fps = convert_fps(frame_rate.unwrap_or(24.0), &mut warnings)?;
    let background_color = background_color.unwrap_or(Rgba {
        r: 255,
        g: 255,
        b: 255,
        a: 255,
    });

    if layers.len() > usize::from(u16::MAX) {
        return Err(ImportError::Invalid(format!(
            "timeline has {} layers; q1s supports at most {}",
            layers.len(),
            u16::MAX
        )));
    }

    let mut imported_keyframes = 0_usize;
    if layers.is_empty() {
        layers.push(ImportedLayer {
            name: "Layer 1".to_string(),
            keyframes: vec![0],
            frame_end: 1,
            kind: LayerKind::Normal,
            masked: false,
            mask_source_index: None,
            parent_index: None,
            hidden: false,
            locked: false,
        });
        max_frame_end = max_frame_end.max(1);
    }

    let parent_kinds: Vec<_> = layers.iter().map(|layer| layer.kind).collect();
    let mut active_masks = std::collections::HashMap::new();
    for (index, layer) in layers.iter_mut().enumerate() {
        let explicit = layer
            .parent_index
            .filter(|parent| parent_kinds.get(*parent) == Some(&LayerKind::Mask));
        layer.mask_source_index = explicit.or_else(|| {
            if layer.masked {
                active_masks.get(&layer.parent_index).copied()
            } else {
                None
            }
        });
        if layer.kind == LayerKind::Mask {
            active_masks.insert(layer.parent_index, index);
        } else if !layer.masked && layer.kind != LayerKind::Folder {
            active_masks.remove(&layer.parent_index);
        }
    }

    for layer in &mut layers {
        layer.keyframes.sort_unstable();
        layer.keyframes.dedup();
        if layer.kind == LayerKind::Folder {
            layer.keyframes.clear();
        } else if layer.keyframes.is_empty() {
            layer.keyframes.push(0);
        }
        imported_keyframes += layer.keyframes.len();
    }

    let frame_count_u64 = max_frame_end.max(1);
    let frame_count = u16::try_from(frame_count_u64).map_err(|_| {
        ImportError::Invalid(format!(
            "timeline has {frame_count_u64} frames; q1s supports at most {}",
            u16::MAX
        ))
    })?;

    let project_name = source_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.trim().is_empty())
        .unwrap_or("untitled")
        .to_string();

    let original_len = layers.len();
    let mut q0_layers = Vec::with_capacity(original_len);
    let mut layer_metadata = std::collections::HashMap::new();
    for (original_index, imported) in layers.iter().enumerate().rev() {
        let layer_id = u16::try_from(original_index + 1)
            .map_err(|_| ImportError::Invalid("layer id overflow".to_string()))?;
        q0_layers.push(Layer {
            layer_id,
            timeline_frame_count: Some(imported.frame_end.max(1).min(u64::from(frame_count)) as u16),
            name: imported.name.clone(),
            explicit_keyframes: imported.keyframes.clone(),
            placements: Vec::new(),
        });

        let parent_folder_id = match imported.parent_index {
            Some(parent_index) if parent_index < original_len => {
                let parent = &layers[parent_index];
                if parent.kind == LayerKind::Folder {
                    Some(u16::try_from(parent_index + 1).map_err(|_| {
                        ImportError::Invalid("parent layer id overflow".to_string())
                    })?)
                } else if parent.kind == LayerKind::Mask {
                    None
                } else {
                    warnings.push(format!(
                        "layer '{}' points at non-folder parent index {}; parent relation was dropped",
                        imported.name, parent_index
                    ));
                    None
                }
            }
            Some(parent_index) => {
                warnings.push(format!(
                    "layer '{}' has missing parent layer index {}; parent relation was dropped",
                    imported.name, parent_index
                ));
                None
            }
            None => None,
        };
        let metadata = LayerMetadata {
            kind: imported.kind,
            parent_folder_id,
            mask_layer_id: imported
                .parent_index
                .filter(|index| {
                    layers
                        .get(*index)
                        .is_some_and(|parent| parent.kind == LayerKind::Mask)
                })
                .or(imported.mask_source_index)
                .and_then(|mask_index| u16::try_from(mask_index + 1).ok()),
            collapsed: false,
            hidden: imported.hidden,
            locked: imported.locked,
        };
        if metadata != LayerMetadata::default() {
            layer_metadata.insert(LayerKey::new(1, layer_id), metadata);
        }
    }

    let mut project = ProjectV2 {
        meta: ProjectMeta {
            name: project_name,
            fps,
            stage_width: width,
            stage_height: height,
            background_color,
            entry_q0rg_id: 1,
        },
        assets: Vec::new(),
        asset_names: std::collections::HashMap::new(),
        asset_appearances: std::collections::HashMap::new(),
        layer_metadata,
        audio_clips: Vec::new(),
        runtime: Default::default(),
        q0rgs: vec![Q0rg {
            q0rg_id: 1,
            name: "Stage".to_string(),
            frame_count,
            script: String::new(),
            layers: q0_layers,
        }],
    };

    normalize_folder_layer_order(&mut project.q0rgs[0], &project.layer_metadata);

    q0s_format::v2::validate(&project).map_err(|error| {
        ImportError::Invalid(format!("converted q1s model failed validation: {error}"))
    })?;

    warnings.push(
        "first q0fla pass imports document/timeline structure only; artwork, symbols, text, media and scripts are not imported yet"
            .to_string(),
    );

    Ok(ImportedProject {
        pending_text: Vec::new(),
        report: ImportReport {
            source_kind: source_kind.to_string(),
            imported_layers: project.q0rgs[0].layers.len(),
            imported_keyframes,
            warnings,
        },
        project,
    })
}

fn import_frame(
    reader: &Reader<&[u8]>,
    element: &BytesStart<'_>,
    layer: &mut ImportedLayer,
    max_frame_end: &mut u64,
) -> Result<(), ImportError> {
    if layer.kind == LayerKind::Folder {
        return Ok(());
    }
    let index = attr(reader, element, b"index")?
        .unwrap_or_else(|| "0".to_string())
        .parse::<u64>()
        .map_err(|_| ImportError::Invalid("DOMFrame has an invalid index".to_string()))?;
    let duration = attr(reader, element, b"duration")?
        .unwrap_or_else(|| "1".to_string())
        .parse::<u64>()
        .map_err(|_| ImportError::Invalid("DOMFrame has an invalid duration".to_string()))?
        .max(1);
    let end = index
        .checked_add(duration)
        .ok_or_else(|| ImportError::Invalid("DOMFrame index/duration overflow".to_string()))?;
    if index >= u64::from(u16::MAX) || end > u64::from(u16::MAX) {
        return Err(ImportError::Invalid(format!(
            "DOMFrame range {index}..{end} exceeds q1s timeline limits"
        )));
    }
    layer.keyframes.push(index as u16);
    layer.frame_end = layer.frame_end.max(end);
    *max_frame_end = (*max_frame_end).max(end);
    Ok(())
}

fn attr(
    reader: &Reader<&[u8]>,
    element: &BytesStart<'_>,
    key: &[u8],
) -> Result<Option<String>, ImportError> {
    for attribute in element.attributes().with_checks(false) {
        let attribute = attribute.map_err(|error| ImportError::Xml(error.to_string()))?;
        if attribute.key.as_ref() == key {
            let value = attribute
                .decode_and_unescape_value(reader.decoder())
                .map_err(|error| ImportError::Xml(error.to_string()))?;
            return Ok(Some(value.into_owned()));
        }
    }
    Ok(None)
}

fn parse_optional_color(
    reader: &Reader<&[u8]>,
    element: &BytesStart<'_>,
    key: &[u8],
) -> Result<Option<Rgba>, ImportError> {
    let Some(value) = attr(reader, element, key)? else {
        return Ok(None);
    };
    let hex = value.strip_prefix('#').unwrap_or(&value);
    if hex.len() != 6 {
        return Err(ImportError::Invalid(format!(
            "DOMDocument attribute {}='{value}' is not an RGB hex colour",
            String::from_utf8_lossy(key)
        )));
    }
    let parse = |range: std::ops::Range<usize>| {
        u8::from_str_radix(&hex[range], 16).map_err(|_| {
            ImportError::Invalid(format!(
                "DOMDocument attribute {}='{value}' is not an RGB hex colour",
                String::from_utf8_lossy(key)
            ))
        })
    };
    Ok(Some(Rgba {
        r: parse(0..2)?,
        g: parse(2..4)?,
        b: parse(4..6)?,
        a: 255,
    }))
}

fn parse_optional_f64(
    reader: &Reader<&[u8]>,
    element: &BytesStart<'_>,
    key: &[u8],
) -> Result<Option<f64>, ImportError> {
    attr(reader, element, key)?
        .map(|value| {
            value.parse::<f64>().map_err(|_| {
                ImportError::Invalid(format!(
                    "DOMDocument attribute {}='{value}' is not a number",
                    String::from_utf8_lossy(key)
                ))
            })
        })
        .transpose()
}

fn convert_dimension(
    value: f64,
    label: &str,
    warnings: &mut Vec<String>,
) -> Result<u16, ImportError> {
    if !value.is_finite() || value <= 0.0 || value > f64::from(u16::MAX) {
        return Err(ImportError::Invalid(format!(
            "stage {label} {value} is outside q1s limits"
        )));
    }
    let rounded = value.round();
    if (rounded - value).abs() > f64::EPSILON {
        warnings.push(format!("stage {label} {value} was rounded to {rounded}"));
    }
    Ok(rounded as u16)
}

fn convert_fps(value: f64, warnings: &mut Vec<String>) -> Result<u16, ImportError> {
    if !value.is_finite() || value <= 0.0 || value > f64::from(u16::MAX) {
        return Err(ImportError::Invalid(format!(
            "frame rate {value} is outside q1s limits"
        )));
    }
    let rounded = value.round();
    if (rounded - value).abs() > f64::EPSILON {
        warnings.push(format!(
            "Animate frame rate {value} fps was rounded to {rounded} fps because q1s currently stores integer fps"
        ));
    }
    Ok(rounded as u16)
}

fn normalize_archive_name(name: &str) -> String {
    name.replace('\\', "/").trim_start_matches('/').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};
    use zip::write::FileOptions;

    const BASIC_XFL: &str = r##"<?xml version="1.0" encoding="utf-8"?>
<DOMDocument xmlns="http://ns.adobe.com/xfl/2008/" width="1280" height="720" frameRate="30">
  <timelines>
    <DOMTimeline name="Scene 1">
      <layers>
        <DOMLayer name="Folder" layerType="folder">
          <frames/>
        </DOMLayer>
        <DOMLayer name="Artwork" parentLayerIndex="0">
          <frames>
            <DOMFrame index="0" duration="5"/>
            <DOMFrame index="5" duration="7"/>
          </frames>
        </DOMLayer>
        <DOMLayer name="Hidden" visible="false" locked="true">
          <frames><DOMFrame index="0" duration="12"/></frames>
        </DOMLayer>
      </layers>
    </DOMTimeline>
  </timelines>
</DOMDocument>"##;

    #[test]
    fn basic_xfl_becomes_a_valid_q1s_project() {
        let imported = import_dom_document(Path::new("demo.fla"), BASIC_XFL.as_bytes(), "test")
            .expect("import xfl");
        let project = &imported.project;
        assert_eq!(project.meta.name, "demo");
        assert_eq!(project.meta.stage_width, 1280);
        assert_eq!(project.meta.stage_height, 720);
        assert_eq!(project.meta.fps, 30);
        assert_eq!(project.q0rgs[0].frame_count, 12);
        assert_eq!(
            project.q0rgs[0]
                .layers
                .iter()
                .map(|layer| layer.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Hidden", "Artwork", "Folder"]
        );
        let artwork = project.q0rgs[0]
            .layers
            .iter()
            .find(|layer| layer.name == "Artwork")
            .unwrap();
        assert_eq!(artwork.explicit_keyframes, vec![0, 5]);
        let artwork_metadata = project.layer_metadata(1, artwork.layer_id);
        assert_eq!(artwork_metadata.parent_folder_id, Some(1));
        let hidden = project.q0rgs[0]
            .layers
            .iter()
            .find(|layer| layer.name == "Hidden")
            .unwrap();
        let hidden_metadata = project.layer_metadata(1, hidden.layer_id);
        assert!(hidden_metadata.hidden);
        assert!(hidden_metadata.locked);
        q0s_format::v2::validate(project).expect("imported project validates");
    }

    #[test]
    fn fla_zip_reads_domdocument_without_extracting_to_disk() {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .start_file("DOMDocument.xml", FileOptions::default())
            .unwrap();
        writer.write_all(BASIC_XFL.as_bytes()).unwrap();
        writer
            .start_file("mimetype", FileOptions::default())
            .unwrap();
        writer.write_all(b"application/vnd.adobe.fla").unwrap();
        let bytes = writer.finish().unwrap().into_inner();

        let root = std::env::temp_dir().join(format!("q0fla-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("movie.fla");
        std::fs::write(&path, bytes).unwrap();
        let imported = import_fla(&path).expect("import synthetic fla");
        assert_eq!(imported.project.meta.name, "movie");
        assert_eq!(imported.project.meta.stage_width, 1280);
        assert_eq!(imported.report.source_kind, "fla");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn adobe_fla_with_inflated_central_directory_size_is_repaired_in_memory() {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .start_file("movie.xfl", FileOptions::default())
            .unwrap();
        writer.write_all(b"PROXY-CS5").unwrap();
        writer
            .start_file("DOMDocument.xml", FileOptions::default())
            .unwrap();
        writer.write_all(BASIC_XFL.as_bytes()).unwrap();
        writer
            .start_file("mimetype", FileOptions::default())
            .unwrap();
        writer.write_all(b"application/vnd.adobe.fla").unwrap();
        let mut bytes = writer.finish().unwrap().into_inner();

        let eocd = bytes
            .windows(4)
            .rposition(|window| window == b"PK\x05\x06")
            .expect("zip eocd");
        let declared_size = u32::from_le_bytes(bytes[eocd + 12..eocd + 16].try_into().unwrap());
        bytes[eocd + 12..eocd + 16]
            .copy_from_slice(&declared_size.saturating_add(54).to_le_bytes());
        assert!(zip::ZipArchive::new(Cursor::new(bytes.clone())).is_err());

        let root =
            std::env::temp_dir().join(format!("q0fla-adobe-zip-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("movie.fla");
        std::fs::write(&path, bytes).unwrap();

        let imported = import_fla(&path).expect("repair animate central directory size");
        assert_eq!(imported.project.meta.stage_width, 1280);
        assert_eq!(imported.project.meta.stage_height, 720);
        assert!(imported
            .report
            .source_kind
            .contains("repaired adobe zip directory"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn fla_import_turns_library_symbol_into_q0rg_with_real_shape_and_background() {
        const STAGE_XFL: &str = r##"<?xml version="1.0" encoding="utf-8"?>
<DOMDocument xmlns="http://ns.adobe.com/xfl/2008/" width="320" height="180" frameRate="24" backgroundColor="#123456">
  <timelines><DOMTimeline name="Scene 1"><layers><DOMLayer name="Stage art"><frames>
    <DOMFrame index="0" duration="4"><elements>
      <DOMSymbolInstance libraryItemName="Leaf" symbolType="graphic" loop="single frame" firstFrame="0">
        <matrix><Matrix tx="12" ty="34"/></matrix>
      </DOMSymbolInstance>
    </elements></DOMFrame>
  </frames></DOMLayer></layers></DOMTimeline></timelines>
</DOMDocument>"##;
        const SYMBOL_XFL: &str = r##"<?xml version="1.0" encoding="utf-8"?>
<DOMSymbolItem xmlns="http://ns.adobe.com/xfl/2008/" name="Leaf" symbolType="graphic">
  <timeline><DOMTimeline name="Leaf"><layers><DOMLayer name="drawing"><frames>
    <DOMFrame index="0" duration="4"><elements><DOMShape>
      <fills><FillStyle index="1"><SolidColor color="#ff0000"/></FillStyle></fills>
      <edges><Edge fillStyle1="1" edges="!0 0|40 0|40 20|0 20|0 0"/></edges>
    </DOMShape></elements></DOMFrame>
  </frames></DOMLayer></layers></DOMTimeline></timeline>
</DOMSymbolItem>"##;

        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .start_file("DOMDocument.xml", FileOptions::default())
            .unwrap();
        writer.write_all(STAGE_XFL.as_bytes()).unwrap();
        writer
            .start_file("LIBRARY/Leaf.xml", FileOptions::default())
            .unwrap();
        writer.write_all(SYMBOL_XFL.as_bytes()).unwrap();
        let bytes = writer.finish().unwrap().into_inner();

        let root = std::env::temp_dir().join(format!("q0fla-symbol-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("symbols.fla");
        std::fs::write(&path, bytes).unwrap();

        let imported = import_fla(&path).expect("import symbol fla");
        assert_eq!(
            imported.project.meta.background_color,
            Rgba {
                r: 0x12,
                g: 0x34,
                b: 0x56,
                a: 255
            }
        );
        assert_eq!(imported.project.q0rgs.len(), 2);
        let leaf = imported
            .project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.name == "Leaf")
            .expect("leaf q0rg");
        assert_eq!(leaf.frame_count, 4);
        assert!(
            !imported.project.assets.is_empty(),
            "leaf shape became vector data"
        );
        let vector = imported
            .project
            .assets
            .iter()
            .find_map(|asset| match asset {
                q0s_format::v2::Asset::Vector(vector) => Some(vector),
                _ => None,
            })
            .expect("native vector asset");
        let points = vector
            .paths
            .iter()
            .flat_map(|path| path.anchors.iter().map(|anchor| anchor.point));
        let (mut min_x, mut min_y, mut max_x, mut max_y) = (
            f32::INFINITY,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NEG_INFINITY,
        );
        for point in points {
            min_x = min_x.min(point.x);
            min_y = min_y.min(point.y);
            max_x = max_x.max(point.x);
            max_y = max_y.max(point.y);
        }
        assert!((min_x - 0.0).abs() < 1.0e-5);
        assert!((min_y - 0.0).abs() < 1.0e-5);
        assert!(
            (max_x - 2.0).abs() < 1.0e-5,
            "40 XFL twips must import as 2 px"
        );
        assert!(
            (max_y - 1.0).abs() < 1.0e-5,
            "20 XFL twips must import as 1 px"
        );
        let stage_placement = &imported.project.q0rgs[0].layers[0].placements[0];
        assert_eq!(
            stage_placement.target,
            q0s_format::v2::Target::Q0rg(leaf.q0rg_id)
        );
        assert_eq!(stage_placement.transform.tx, 12.0);
        assert_eq!(stage_placement.transform.ty, 34.0);
        assert_eq!(
            stage_placement.fx.q0rg_playback.mode,
            q0s_format::v2::Q0rgPlaybackMode::SingleFrame
        );
        q0s_format::v2::validate(&imported.project).expect("imported symbol project validates");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn fla_import_resolves_palette_bitmap_through_library_symbol_placement() {
        use std::io::Write as _;

        const STAGE_XFL: &str = r##"<DOMDocument xmlns="http://ns.adobe.com/xfl/2008/" width="320" height="180" frameRate="24">
  <media><DOMBitmapItem name="Media/Palette" bitmapDataHRef="M 1.dat"/></media>
  <timelines><DOMTimeline name="Scene 1"><layers><DOMLayer name="art"><frames>
    <DOMFrame index="0" duration="1"><elements><DOMSymbolInstance libraryItemName="Leaf"/></elements></DOMFrame>
  </frames></DOMLayer></layers></DOMTimeline></timelines>
</DOMDocument>"##;
        const SYMBOL_XFL: &str = r##"<DOMSymbolItem xmlns="http://ns.adobe.com/xfl/2008/" name="Leaf">
  <timeline><DOMTimeline name="Leaf"><layers><DOMLayer name="bitmap"><frames>
    <DOMFrame index="0"><elements><DOMBitmapInstance libraryItemName="Media/Palette"/></elements></DOMFrame>
  </frames></DOMLayer></layers></DOMTimeline></timeline>
</DOMSymbolItem>"##;

        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&[0u8]).unwrap();
        let compressed = encoder.finish().unwrap();
        let mut dat = Vec::new();
        dat.extend_from_slice(&[0x03, 0x03]);
        dat.extend_from_slice(&1u16.to_le_bytes());
        dat.extend_from_slice(&1u16.to_le_bytes());
        dat.extend_from_slice(&1u16.to_le_bytes());
        dat.extend_from_slice(&[0u8; 16]);
        dat.extend_from_slice(&[1, 1, 0, 0]);
        dat.extend_from_slice(&[240, 120, 30, 96]);
        dat.extend_from_slice(&(compressed.len() as u16).to_le_bytes());
        dat.extend_from_slice(&compressed);
        dat.extend_from_slice(&0u16.to_le_bytes());

        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .start_file("DOMDocument.xml", FileOptions::default())
            .unwrap();
        writer.write_all(STAGE_XFL.as_bytes()).unwrap();
        writer
            .start_file("LIBRARY/Leaf.xml", FileOptions::default())
            .unwrap();
        writer.write_all(SYMBOL_XFL.as_bytes()).unwrap();
        writer
            .start_file("bin/M 1.dat", FileOptions::default())
            .unwrap();
        writer.write_all(&dat).unwrap();
        let bytes = writer.finish().unwrap().into_inner();

        let root =
            std::env::temp_dir().join(format!("q0fla-indexed-bitmap-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("indexed-bitmap.fla");
        std::fs::write(&path, bytes).unwrap();

        let imported = import_fla(&path).expect("import indexed-bitmap fla");
        assert!(!imported
            .report
            .warnings
            .iter()
            .any(|warning| warning.contains("missing bitmap")));
        let bitmap = imported
            .project
            .assets
            .iter()
            .find_map(|asset| match asset {
                q0s_format::v2::Asset::Bitmap(bitmap) => Some(bitmap),
                _ => None,
            })
            .expect("palette bitmap asset");
        assert_eq!(bitmap.rgba, vec![240, 120, 30, 96]);
        let leaf = imported
            .project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.name == "Leaf")
            .expect("library symbol q0rg");
        assert!(matches!(
            leaf.layers[0].placements[0].target,
            q0s_format::v2::Target::Asset(_)
        ));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn fla_motion_tween_fx_and_nested_playback_survive_q1s_roundtrip() {
        const STAGE_XFL: &str = r##"<DOMDocument xmlns="http://ns.adobe.com/xfl/2008/" width="320" height="180" frameRate="24">
  <timelines><DOMTimeline name="Scene 1"><layers><DOMLayer name="instances"><frames>
    <DOMFrame index="0" duration="20"><elements><DOMSymbolInstance libraryItemName="Outer" symbolType="graphic" loop="loop" firstFrame="2"/></elements></DOMFrame>
  </frames></DOMLayer></layers></DOMTimeline></timelines>
</DOMDocument>"##;
        const OUTER_XFL: &str = r##"<DOMSymbolItem xmlns="http://ns.adobe.com/xfl/2008/" name="Outer">
  <timeline><DOMTimeline name="Outer"><layers><DOMLayer name="motion"><frames>
    <DOMFrame index="0" duration="10" tweenType="motion"><tweens><CustomEase target="all"><Point x="0" y="0"/><Point x="0.2" y="0"/><Point x="0.7" y="0.4"/><Point x="1" y="1"/></CustomEase></tweens><elements>
      <DOMSymbolInstance libraryItemName="Spark" symbolType="graphic" loop="loop" firstFrame="1"><matrix><Matrix a="1" d="1" tx="10" ty="20"/></matrix><color><Color alphaMultiplier="0.2"/></color><filters><BlurFilter blurX="2" blurY="4" quality="2"/><GlowFilter blurX="3" blurY="5" color="#FF8040" quality="3" strength="1"/></filters></DOMSymbolInstance>
    </elements></DOMFrame>
    <DOMFrame index="10" duration="10"><elements>
      <DOMSymbolInstance libraryItemName="Spark" symbolType="graphic" loop="loop" firstFrame="1"><matrix><Matrix a="2.1213203" b="2.1213203" c="-1.4142135" d="1.4142135" tx="110" ty="60"/></matrix><color><Color alphaMultiplier="1"/></color><filters><BlurFilter blurX="20" blurY="40" quality="2"/><GlowFilter blurX="13" blurY="25" color="#4080FF" quality="3" strength="3"/></filters></DOMSymbolInstance>
    </elements></DOMFrame>
  </frames></DOMLayer></layers></DOMTimeline></timeline>
</DOMSymbolItem>"##;
        const SPARK_XFL: &str = r##"<DOMSymbolItem xmlns="http://ns.adobe.com/xfl/2008/" name="Spark">
  <timeline><DOMTimeline name="Spark"><layers><DOMLayer name="content"><frames><DOMFrame index="0" duration="4"/></frames></DOMLayer></layers></DOMTimeline></timeline>
</DOMSymbolItem>"##;

        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, contents) in [
            ("DOMDocument.xml", STAGE_XFL),
            ("LIBRARY/Outer.xml", OUTER_XFL),
            ("LIBRARY/Spark.xml", SPARK_XFL),
        ] {
            writer.start_file(name, FileOptions::default()).unwrap();
            writer.write_all(contents.as_bytes()).unwrap();
        }
        let bytes = writer.finish().unwrap().into_inner();
        let root =
            std::env::temp_dir().join(format!("q0fla-motion-tween-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("motion-tween.fla");
        std::fs::write(&path, bytes).unwrap();

        let imported = import_fla(&path).expect("import motion tween fla");
        let project = &imported.project;
        // The stage symbol takes its name from the imported scene.
        let stage = project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == project.meta.entry_q0rg_id)
            .unwrap();
        let outer = project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.name == "Outer")
            .unwrap();
        let spark = project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.name == "Spark")
            .unwrap();
        let stage_placement = &stage.layers[0].placements[0];
        assert_eq!(
            stage_placement.target,
            q0s_format::v2::Target::Q0rg(outer.q0rg_id)
        );
        assert_eq!(
            q0s_format::v2::resolve_q0rg_playback_frame(
                project,
                outer.q0rg_id,
                0,
                stage_placement.frame,
                stage_placement.fx.q0rg_playback,
            ),
            2,
            "graphic symbol starts at its authored firstFrame"
        );

        let layer = &outer.layers[0];
        assert_eq!(layer.explicit_keyframes, vec![0, 10]);
        let source = &layer.placements[0];
        assert_eq!(source.target, q0s_format::v2::Target::Q0rg(spark.q0rg_id));
        assert_eq!(source.tween.to_frame(), Some(10));
        let q0s_format::v2::Tween::Eased { easing, .. } = source.tween else {
            panic!("Animate custom ease must survive full FLA import");
        };
        let t = easing.sample(0.5);
        assert!(
            (t - 0.5).abs() > 0.02,
            "fixture must exercise non-linear easing"
        );

        let state = q0s_format::raster::active_placement_states_at(layer, 5)
            .into_iter()
            .next()
            .expect("interpolated Spark placement");
        assert!((state.transform.tx - (10.0 + 100.0 * t)).abs() < 1.0e-3);
        assert!((state.transform.ty - (20.0 + 40.0 * t)).abs() < 1.0e-3);
        assert!((state.transform.sx - (1.0 + 2.0 * t)).abs() < 1.0e-3);
        assert!((state.transform.sy - (1.0 + t)).abs() < 1.0e-3);
        assert!((state.transform.rotation - (std::f32::consts::FRAC_PI_4 * t)).abs() < 1.0e-3);
        assert!((state.fx.opacity - (0.2 + 0.8 * t)).abs() < 1.0e-3);
        let blur = state.fx.blur.expect("interpolated blur");
        assert!((blur.radius_x - (2.0 + 18.0 * t)).abs() < 1.0e-3);
        assert!((blur.radius_y - (4.0 + 36.0 * t)).abs() < 1.0e-3);
        let glow = state.fx.glow.expect("interpolated glow");
        assert!((glow.radius_x - (3.0 + 10.0 * t)).abs() < 1.0e-3);
        assert!((glow.radius_y - (5.0 + 20.0 * t)).abs() < 1.0e-3);
        assert!((glow.strength - (1.0 + 2.0 * t)).abs() < 1.0e-3);
        assert_eq!(
            q0s_format::v2::resolve_q0rg_playback_frame(
                project,
                spark.q0rg_id,
                5,
                source.frame,
                source.fx.q0rg_playback,
            ),
            3,
            "nested graphic placement keeps its authored loop range"
        );

        let held = q0s_format::raster::active_placement_states_at(layer, 19)
            .into_iter()
            .next()
            .expect("held destination placement");
        assert!((held.transform.tx - 110.0).abs() < 1.0e-3);
        assert!((held.transform.sx - 3.0).abs() < 1.0e-3);
        assert!((held.fx.opacity - 1.0).abs() < 1.0e-6);

        q0s_format::v2::validate(project).expect("imported tween project validates");
        let roundtrip = q0s_format::v2::parse(
            &q0s_format::v2::write(project).expect("write imported q1s project"),
        )
        .expect("parse imported q1s project");
        assert!(q0s_format::v2::wire_equivalent(project, &roundtrip));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn fla_shape_tween_morphs_drawn_shapes_and_survives_q1s_roundtrip() {
        // A 10px red square that becomes a 30px blue one over ten frames, with
        // an Animate custom ease. Edge coordinates are in twips (20 per pixel).
        const STAGE_XFL: &str = r##"<DOMDocument xmlns="http://ns.adobe.com/xfl/2008/" width="320" height="180" frameRate="24">
  <timelines><DOMTimeline name="Scene 1"><layers><DOMLayer name="morph"><frames>
    <DOMFrame index="0" duration="10" tweenType="shape"><tweens><CustomEase target="all"><Point x="0" y="0"/><Point x="0.2" y="0"/><Point x="0.7" y="0.4"/><Point x="1" y="1"/></CustomEase></tweens><elements>
      <DOMShape><fills><FillStyle index="1"><SolidColor color="#FF0000"/></FillStyle></fills><edges><Edge fillStyle1="1" edges="!0 0|200 0|200 200|0 200|0 0"/></edges></DOMShape>
      <DOMSymbolInstance libraryItemName="Spark" symbolType="graphic" loop="loop"/>
    </elements></DOMFrame>
    <DOMFrame index="10" duration="10"><elements>
      <DOMShape><fills><FillStyle index="1"><SolidColor color="#0000FF"/></FillStyle></fills><edges><Edge fillStyle1="1" edges="!0 0|600 0|600 600|0 600|0 0"/></edges></DOMShape>
    </elements></DOMFrame>
  </frames></DOMLayer></layers></DOMTimeline></timelines>
</DOMDocument>"##;
        const SPARK_XFL: &str = r##"<DOMSymbolItem xmlns="http://ns.adobe.com/xfl/2008/" name="Spark">
  <timeline><DOMTimeline name="Spark"><layers><DOMLayer name="content"><frames><DOMFrame index="0" duration="4"/></frames></DOMLayer></layers></DOMTimeline></timeline>
</DOMSymbolItem>"##;

        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, contents) in [
            ("DOMDocument.xml", STAGE_XFL),
            ("LIBRARY/Spark.xml", SPARK_XFL),
        ] {
            writer.start_file(name, FileOptions::default()).unwrap();
            writer.write_all(contents.as_bytes()).unwrap();
        }
        let bytes = writer.finish().unwrap().into_inner();
        let root =
            std::env::temp_dir().join(format!("q0fla-shape-tween-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("shape-tween.fla");
        std::fs::write(&path, bytes).unwrap();

        let imported = import_fla(&path).expect("import shape tween fla");
        let project = &imported.project;
        let stage = project
            .q0rgs
            .iter()
            .find(|q0rg| q0rg.q0rg_id == project.meta.entry_q0rg_id)
            .unwrap();
        let layer = &stage.layers[0];
        let is_drawing = |placement: &q0s_format::v2::Placement| {
            matches!(placement.target, q0s_format::v2::Target::Asset(_))
        };

        let source = layer
            .placements
            .iter()
            .position(|placement| placement.frame == 0 && is_drawing(placement))
            .expect("drawn shape on the first keyframe");
        let q0s_format::v2::Tween::Shape { to_frame, easing } = layer.placements[source].tween
        else {
            panic!("Animate shape tween did not become a native shape tween");
        };
        assert_eq!(to_frame, 10);
        assert!(matches!(easing, q0s_format::v2::Easing::CubicBezier { .. }));
        // The symbol sharing that keyframe is held, not tweened.
        let instance = layer
            .placements
            .iter()
            .find(|placement| placement.frame == 0 && !is_drawing(placement))
            .expect("symbol instance on the first keyframe");
        assert_eq!(instance.tween, q0s_format::v2::Tween::None);
        assert!(layer
            .placements
            .iter()
            .filter(|placement| placement.frame == 10)
            .all(|placement| placement.tween == q0s_format::v2::Tween::None));

        // Linear progress at frame 5 would be exactly 20px wide; the ease
        // makes it something else in between, and the colour follows.
        let morph = q0s_format::shape_tween::project_layer_morph(project, layer, 5)
            .expect("frame 5 is inside the shape tween");
        let shape = &morph.for_placement(source).expect("morphed drawing")[0];
        let width = shape.paths[0]
            .anchors
            .iter()
            .map(|anchor| anchor.point.x)
            .fold(f32::NEG_INFINITY, f32::max);
        assert!(width > 10.5 && width < 29.5, "in-between width {width}");
        assert!((width - 20.0).abs() > 0.5, "the custom ease is applied");
        let fill = shape.fill.expect("in-between fill");
        assert!(fill.r > 0 && fill.r < 255 && fill.b > 0 && fill.b < 255);
        assert_eq!(u16::from(fill.r) + u16::from(fill.b), 255);

        q0s_format::v2::validate(project).expect("imported shape tween project validates");
        let roundtrip = q0s_format::v2::parse(
            &q0s_format::v2::write(project).expect("write imported q1s project"),
        )
        .expect("parse imported q1s project");
        assert!(q0s_format::v2::wire_equivalent(project, &roundtrip));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn xfl_marker_uses_domdocument_beside_it() {
        let root = std::env::temp_dir().join(format!("q0xfl-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let marker = root.join("project.xfl");
        std::fs::write(&marker, b"PROXY-CS5").unwrap();
        std::fs::write(root.join("DOMDocument.xml"), BASIC_XFL).unwrap();
        let imported = import_xfl(&marker).expect("import uncompressed xfl");
        assert_eq!(imported.project.meta.stage_height, 720);
        let _ = std::fs::remove_dir_all(root);
    }
}
