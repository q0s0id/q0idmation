use std::collections::{HashMap, HashSet};

use crate::error::Error;
use crate::io::{write_string_u16, Cursor};
use crate::model::{Background, Bitmap, Header, Movie, Placement, SUPPORTED_VERSION};
use crate::q0s_writer::write_q0s;

pub const Q1S_MAGIC: [u8; 4] = *b"Q1S\0";
pub const Q1S_VERSION: u16 = 1;
const ASSET_KIND_BITMAP_RGBA: u8 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Q1ProjectMeta {
    pub name: String,
    pub fps: u16,
    pub stage_width: u16,
    pub stage_height: u16,
    pub entry_scene_id: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Q1AssetBitmap {
    pub asset_id: u16,
    pub width: u16,
    pub height: u16,
    pub rgba: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Q1Placement {
    pub frame: u16,
    pub asset_id: u16,
    pub x: i16,
    pub y: i16,
    pub scale_x: f32,
    pub scale_y: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Q1Layer {
    pub layer_id: u16,
    pub name: String,
    pub placements: Vec<Q1Placement>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Q1Scene {
    pub scene_id: u16,
    pub name: String,
    pub frame_count: u16,
    pub layers: Vec<Q1Layer>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Q1Project {
    pub meta: Q1ProjectMeta,
    pub assets: Vec<Q1AssetBitmap>,
    pub scenes: Vec<Q1Scene>,
}

pub fn validate_q1s(project: &Q1Project) -> Result<(), Error> {
    if project.meta.fps == 0 {
        return Err(Error::Validation("fps must be > 0"));
    }
    if project.meta.stage_width == 0 || project.meta.stage_height == 0 {
        return Err(Error::Validation("stage size must be > 0"));
    }
    if project.scenes.is_empty() {
        return Err(Error::Validation("at least one scene is required"));
    }

    let mut asset_ids = HashSet::new();
    for asset in &project.assets {
        if !asset_ids.insert(asset.asset_id) {
            return Err(Error::Validation("asset_id must be unique"));
        }
        let expected = usize::from(asset.width)
            .saturating_mul(usize::from(asset.height))
            .saturating_mul(4);
        if asset.rgba.len() != expected {
            return Err(Error::Validation(
                "asset rgba length must match width * height * 4",
            ));
        }
    }

    let mut scene_ids = HashSet::new();
    for scene in &project.scenes {
        if !scene_ids.insert(scene.scene_id) {
            return Err(Error::Validation("scene_id must be unique"));
        }
        if scene.frame_count == 0 {
            return Err(Error::Validation("scene frame_count must be > 0"));
        }

        let mut layer_ids = HashSet::new();
        for layer in &scene.layers {
            if !layer_ids.insert(layer.layer_id) {
                return Err(Error::Validation("layer_id must be unique within scene"));
            }
            for placement in &layer.placements {
                if placement.frame >= scene.frame_count {
                    return Err(Error::Validation(
                        "placement frame is out of scene frame_count bounds",
                    ));
                }
                if !placement.scale_x.is_finite() || !placement.scale_y.is_finite() {
                    return Err(Error::Validation("placement scale must be finite"));
                }
                if placement.scale_x <= 0.0 || placement.scale_y <= 0.0 {
                    return Err(Error::Validation("placement scale must be positive"));
                }
                if !asset_ids.contains(&placement.asset_id) {
                    return Err(Error::Validation("placement references unknown asset_id"));
                }
            }
        }
    }

    if !scene_ids.contains(&project.meta.entry_scene_id) {
        return Err(Error::Validation(
            "entry_scene_id must reference an existing scene",
        ));
    }

    Ok(())
}

pub fn write_q1s(project: &Q1Project) -> Result<Vec<u8>, Error> {
    validate_q1s(project)?;

    let asset_count = u16::try_from(project.assets.len())
        .map_err(|_| Error::Overflow("asset count exceeds u16"))?;
    let scene_count = u16::try_from(project.scenes.len())
        .map_err(|_| Error::Overflow("scene count exceeds u16"))?;

    let mut out = Vec::new();
    out.extend_from_slice(&Q1S_MAGIC);
    out.extend_from_slice(&Q1S_VERSION.to_le_bytes());
    out.extend_from_slice(&0_u16.to_le_bytes()); // reserved flags

    write_string_u16(&mut out, &project.meta.name)?;
    out.extend_from_slice(&project.meta.fps.to_le_bytes());
    out.extend_from_slice(&project.meta.stage_width.to_le_bytes());
    out.extend_from_slice(&project.meta.stage_height.to_le_bytes());
    out.extend_from_slice(&project.meta.entry_scene_id.to_le_bytes());
    out.extend_from_slice(&asset_count.to_le_bytes());
    out.extend_from_slice(&scene_count.to_le_bytes());

    let mut assets: Vec<&Q1AssetBitmap> = project.assets.iter().collect();
    assets.sort_by_key(|a| a.asset_id);
    for asset in assets {
        let payload_len = u32::try_from(asset.rgba.len())
            .map_err(|_| Error::Overflow("asset payload length exceeds u32"))?;

        out.extend_from_slice(&asset.asset_id.to_le_bytes());
        out.push(ASSET_KIND_BITMAP_RGBA);
        out.extend_from_slice(&asset.width.to_le_bytes());
        out.extend_from_slice(&asset.height.to_le_bytes());
        out.extend_from_slice(&payload_len.to_le_bytes());
        out.extend_from_slice(&asset.rgba);
    }

    let mut scenes: Vec<&Q1Scene> = project.scenes.iter().collect();
    scenes.sort_by_key(|s| s.scene_id);
    for scene in scenes {
        let layer_count = u16::try_from(scene.layers.len())
            .map_err(|_| Error::Overflow("layer count exceeds u16"))?;

        out.extend_from_slice(&scene.scene_id.to_le_bytes());
        write_string_u16(&mut out, &scene.name)?;
        out.extend_from_slice(&scene.frame_count.to_le_bytes());
        out.extend_from_slice(&layer_count.to_le_bytes());

        let mut layers: Vec<&Q1Layer> = scene.layers.iter().collect();
        layers.sort_by_key(|l| l.layer_id);
        for layer in layers {
            let placement_count = u16::try_from(layer.placements.len())
                .map_err(|_| Error::Overflow("placement count exceeds u16"))?;

            out.extend_from_slice(&layer.layer_id.to_le_bytes());
            write_string_u16(&mut out, &layer.name)?;
            out.extend_from_slice(&placement_count.to_le_bytes());

            let mut placements: Vec<&Q1Placement> = layer.placements.iter().collect();
            placements.sort_by_key(|p| (p.frame, p.asset_id, p.x, p.y));
            for p in placements {
                out.extend_from_slice(&p.frame.to_le_bytes());
                out.extend_from_slice(&p.asset_id.to_le_bytes());
                out.extend_from_slice(&p.x.to_le_bytes());
                out.extend_from_slice(&p.y.to_le_bytes());
                out.extend_from_slice(&p.scale_x.to_le_bytes());
                out.extend_from_slice(&p.scale_y.to_le_bytes());
            }
        }
    }

    Ok(out)
}

pub fn parse_q1s(bytes: &[u8]) -> Result<Q1Project, Error> {
    let mut c = Cursor::new(bytes);
    let magic = c.read_exact(4)?;
    let mut magic_arr = [0_u8; 4];
    magic_arr.copy_from_slice(magic);
    if magic_arr != Q1S_MAGIC {
        return Err(Error::InvalidMagic(magic_arr));
    }

    let version = c.read_u16()?;
    if version != Q1S_VERSION {
        return Err(Error::UnsupportedVersion(version));
    }
    let flags = c.read_u16()?;
    if flags != 0 {
        return Err(Error::UnsupportedFlags(flags));
    }

    let name = c.read_string_u16()?;
    let fps = c.read_u16()?;
    let stage_width = c.read_u16()?;
    let stage_height = c.read_u16()?;
    let entry_scene_id = c.read_u16()?;
    let asset_count = c.read_u16()?;
    let scene_count = c.read_u16()?;

    let mut assets = Vec::with_capacity(usize::from(asset_count));
    for _ in 0..asset_count {
        let asset_id = c.read_u16()?;
        let kind = c.read_u8()?;
        if kind != ASSET_KIND_BITMAP_RGBA {
            return Err(Error::InvalidAssetKind(kind));
        }
        let width = c.read_u16()?;
        let height = c.read_u16()?;
        let len = usize::try_from(c.read_u32()?)
            .map_err(|_| Error::Validation("asset payload length does not fit usize"))?;
        let rgba = c.read_exact(len)?.to_vec();
        assets.push(Q1AssetBitmap {
            asset_id,
            width,
            height,
            rgba,
        });
    }

    let mut scenes = Vec::with_capacity(usize::from(scene_count));
    for _ in 0..scene_count {
        let scene_id = c.read_u16()?;
        let scene_name = c.read_string_u16()?;
        let frame_count = c.read_u16()?;
        let layer_count = c.read_u16()?;
        let mut layers = Vec::with_capacity(usize::from(layer_count));
        for _ in 0..layer_count {
            let layer_id = c.read_u16()?;
            let layer_name = c.read_string_u16()?;
            let placement_count = c.read_u16()?;
            let mut placements = Vec::with_capacity(usize::from(placement_count));
            for _ in 0..placement_count {
                placements.push(Q1Placement {
                    frame: c.read_u16()?,
                    asset_id: c.read_u16()?,
                    x: c.read_i16()?,
                    y: c.read_i16()?,
                    scale_x: c.read_f32()?,
                    scale_y: c.read_f32()?,
                });
            }
            layers.push(Q1Layer {
                layer_id,
                name: layer_name,
                placements,
            });
        }
        scenes.push(Q1Scene {
            scene_id,
            name: scene_name,
            frame_count,
            layers,
        });
    }

    c.finish()?;

    let project = Q1Project {
        meta: Q1ProjectMeta {
            name,
            fps,
            stage_width,
            stage_height,
            entry_scene_id,
        },
        assets,
        scenes,
    };
    validate_q1s(&project)?;
    Ok(project)
}

pub fn q1s_to_movie(project: &Q1Project) -> Result<Movie, Error> {
    validate_q1s(project)?;

    let scene = project
        .scenes
        .iter()
        .find(|s| s.scene_id == project.meta.entry_scene_id)
        .ok_or(Error::Validation("entry scene is missing"))?;

    let mut bitmaps = HashMap::new();
    for asset in &project.assets {
        bitmaps.insert(
            asset.asset_id,
            Bitmap {
                id: asset.asset_id,
                width: asset.width,
                height: asset.height,
                rgba: asset.rgba.clone(),
            },
        );
    }

    let mut placements_by_frame = vec![Vec::<Placement>::new(); usize::from(scene.frame_count)];
    for layer in &scene.layers {
        for p in &layer.placements {
            placements_by_frame[usize::from(p.frame)].push(Placement {
                frame: p.frame,
                bitmap_id: p.asset_id,
                x: p.x,
                y: p.y,
                scale_x: p.scale_x,
                scale_y: p.scale_y,
            });
        }
    }

    Ok(Movie {
        header: Header {
            version: SUPPORTED_VERSION,
            fps: project.meta.fps,
            frame_count: scene.frame_count,
        },
        background: Background {
            r: 20,
            g: 26,
            b: 38,
            a: 255,
        },
        bitmaps,
        placements_by_frame,
    })
}

pub fn q1s_to_q0s_bytes(project: &Q1Project) -> Result<Vec<u8>, Error> {
    let movie = q1s_to_movie(project)?;
    write_q0s(&movie)
}
