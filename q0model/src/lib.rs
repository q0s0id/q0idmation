//! 3D models for q0idmation: reading FBX, glTF and OBJ files, ready-made
//! shapes, and the mesh operations of the model editor.

use std::path::Path;

use q0s_format::three::ModelData;

pub mod fbx;
pub mod gltf;
pub mod obj;
pub mod ops;
pub mod prim;

/// A model read from a file.
#[derive(Debug, Clone)]
pub struct Imported {
    pub model: ModelData,
    pub name: String,
    /// Things that could not be brought in, such as missing textures.
    pub warnings: Vec<String>,
}

/// File extensions that import as models.
pub const EXTENSIONS: &[&str] = &["fbx", "glb", "gltf", "obj"];

pub fn is_model_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

/// Reads a model file, sized to fit about `fit_pixels` on the stage.
pub fn import_path(path: &Path, fit_pixels: f32) -> Result<Imported, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let mut imported = import_bytes(&bytes, &extension, path.parent())?;
    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
        imported.name = stem.to_string();
    }
    fit(&mut imported.model, fit_pixels);
    Ok(imported)
}

/// Reads model bytes of a known kind; `base_dir` is where files it names
/// (textures, buffers) are looked for.
pub fn import_bytes(
    bytes: &[u8],
    extension: &str,
    base_dir: Option<&Path>,
) -> Result<Imported, String> {
    match extension {
        "fbx" => fbx::import(bytes, base_dir),
        "glb" | "gltf" => gltf::import(bytes, base_dir),
        "obj" => obj::import(bytes, base_dir),
        other => Err(format!("{other} files are not 3D models q0editor reads")),
    }
}

/// Sets the stage size so the model's largest side spans `pixels`.
pub fn fit(model: &mut ModelData, pixels: f32) {
    if let Some((lo, hi)) = model.bounds() {
        let largest = (hi - lo).max_elem();
        if largest > 1.0e-6 && largest.is_finite() {
            model.scale = pixels / largest;
        }
    }
}

/// A fresh model holding one shape.
pub fn new_model(primitive: prim::Primitive, fit_pixels: f32) -> ModelData {
    let mut model = ModelData::default();
    let mut node = q0s_format::three::Node::new(primitive.label());
    node.parent = None;
    model.nodes.push(node);
    let mut mesh = primitive.build(2.0, 12);
    mesh.node = Some(0);
    model.meshes.push(mesh);
    fit(&mut model, fit_pixels);
    model
}
