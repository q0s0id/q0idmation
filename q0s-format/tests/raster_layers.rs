//! Raster layers in the file format: round trips, shared tiles, versions,
//! and drawing them.

use std::collections::HashMap;
use std::sync::Arc;

use q0s_format::pixels::{PixelRect, RasterAsset};
use q0s_format::v2::{
    parse, write, write_packed, Asset, Layer, LayerKey, LayerMetadata, Placement, PlacementFx,
    ProjectMeta, ProjectV2, Q0rg, Rgba, Target, Transform2D, Tween, Q1S_VERSION_CAMERA_LENGTHS,
    Q1S_VERSION_RASTER,
};

fn placement(frame: u16, asset_id: u16) -> Placement {
    Placement {
        instance_id: 0,
        frame,
        target: Target::Asset(asset_id),
        transform: Transform2D::IDENTITY,
        pivot: None,
        tween: Tween::None,
        fx: PlacementFx::default(),
    }
}

/// A red square of `side` px with its corner at `(x, y)`.
fn square(asset_id: u16, x: i32, y: i32, side: i32) -> RasterAsset {
    let rect = PixelRect::new(x, y, x + side, y + side);
    let rgba = [255_u8, 0, 0, 255].repeat((side * side) as usize);
    RasterAsset::from_pixels(asset_id, rect, &rgba)
}

fn project(assets: Vec<Asset>, placements: Vec<Placement>, raster_layer: bool) -> ProjectV2 {
    let mut layer_metadata = HashMap::new();
    if raster_layer {
        layer_metadata.insert(
            LayerKey::new(1, 1),
            LayerMetadata {
                raster: true,
                ..LayerMetadata::default()
            },
        );
    }
    ProjectV2 {
        meta: ProjectMeta {
            name: "raster".into(),
            fps: 24,
            stage_width: 320,
            stage_height: 200,
            background_color: Rgba {
                r: 255,
                g: 255,
                b: 255,
                a: 255,
            },
            entry_q0rg_id: 1,
        },
        assets,
        asset_names: HashMap::new(),
        asset_appearances: HashMap::new(),
        layer_metadata,
        audio_clips: Vec::new(),
        runtime: Default::default(),
        q0rgs: vec![Q0rg {
            q0rg_id: 1,
            name: "stage".into(),
            frame_count: 12,
            script: String::new(),
            layers: vec![Layer {
                layer_id: 1,
                timeline_frame_count: None,
                name: "Ink".into(),
                explicit_keyframes: vec![0],
                placements,
            }],
        }],
    }
}

fn version(bytes: &[u8]) -> u16 {
    u16::from_le_bytes([bytes[4], bytes[5]])
}

fn with_scene_table(project: &ProjectV2) -> ProjectV2 {
    let mut expected = project.clone();
    expected.runtime.scenes = project.scenes();
    expected
}

#[test]
fn a_raster_layer_round_trips_plain_and_packed() {
    let first = square(1, -20, 10, 90);
    let mut second = first.clone();
    second.asset_id = 2;
    // The second drawing shares all tiles but one, which it paints over.
    let tile = second.tile_mut(0, 0).unwrap();
    tile.pixels_mut()[..4].copy_from_slice(&[0, 0, 200, 200]);
    let project = project(
        vec![Asset::Raster(first), Asset::Raster(second)],
        vec![placement(0, 1), placement(4, 2)],
        true,
    );
    for bytes in [write(&project).unwrap(), write_packed(&project).unwrap()] {
        let back = parse(&bytes).unwrap();
        assert_eq!(back, with_scene_table(&project));
        assert!(back.layer_metadata[&LayerKey::new(1, 1)].raster);
    }
    let bytes = write(&project).unwrap();
    assert_eq!(version(&bytes), Q1S_VERSION_RASTER);
    // Tiles the two drawings share come back shared, not as copies.
    let back = parse(&bytes).unwrap();
    let (Asset::Raster(a), Asset::Raster(b)) = (&back.assets[0], &back.assets[1]) else {
        panic!("raster assets come back as raster assets");
    };
    let shared = a
        .tiles
        .iter()
        .filter(|(key, tile)| Arc::ptr_eq(tile, &b.tiles[key]))
        .count();
    assert_eq!(shared, a.tiles.len() - 1);
}

#[test]
fn a_held_drawing_costs_almost_nothing_to_store_twice() {
    let drawing = square(1, 0, 0, 256);
    let once = project(
        vec![Asset::Raster(drawing.clone())],
        vec![placement(0, 1)],
        true,
    );
    let mut copy = drawing.clone();
    copy.asset_id = 2;
    let twice = project(
        vec![Asset::Raster(drawing), Asset::Raster(copy)],
        vec![placement(0, 1), placement(6, 2)],
        true,
    );
    let once = write(&once).unwrap().len();
    let twice = write(&twice).unwrap().len();
    // 16 tiles: a 9 byte reference each, plus the asset header and the
    // second keyframe's placement.
    assert!(twice - once < 16 * 9 + 80, "{once} then {twice}");
}

#[test]
fn projects_without_raster_keep_the_older_version() {
    let plain = project(Vec::new(), Vec::new(), false);
    let bytes = write(&plain).unwrap();
    assert!(version(&bytes) < Q1S_VERSION_RASTER);
    assert!(version(&bytes) <= Q1S_VERSION_CAMERA_LENGTHS);
    let back = parse(&bytes).unwrap();
    assert!(back
        .layer_metadata
        .values()
        .all(|metadata| !metadata.raster));
    // An empty raster layer alone is enough to need the new version.
    let empty_raster = project(Vec::new(), Vec::new(), true);
    let bytes = write(&empty_raster).unwrap();
    assert_eq!(version(&bytes), Q1S_VERSION_RASTER);
    assert_eq!(parse(&bytes).unwrap(), with_scene_table(&empty_raster));
}

#[test]
fn a_newer_body_version_is_refused_by_name() {
    let mut bytes = write(&project(Vec::new(), Vec::new(), true)).unwrap();
    let newer = q0s_format::v2::Q1S_VERSION_CURRENT + 1;
    bytes[4..6].copy_from_slice(&newer.to_le_bytes());
    assert_eq!(
        parse(&bytes),
        Err(q0s_format::Error::UnsupportedVersion(newer))
    );
}

#[test]
fn damaged_tiles_are_refused() {
    let project = project(
        vec![Asset::Raster(square(1, 0, 0, 8))],
        vec![placement(0, 1)],
        true,
    );
    let bytes = write(&project).unwrap();
    for cut in [bytes.len() / 2, bytes.len() - 40] {
        let mut broken = bytes.clone();
        for byte in &mut broken[cut..cut + 6] {
            *byte ^= 0x5a;
        }
        assert!(parse(&broken).is_err() || parse(&broken).unwrap() != project);
    }
}

#[test]
fn raster_drawings_render_in_export() {
    let mut drawing = square(1, 10, 20, 30);
    // A half-transparent blue pixel, premultiplied.
    drawing.put(PixelRect::new(50, 50, 51, 51), &[0, 0, 128, 128]);
    let project = project(vec![Asset::Raster(drawing)], vec![placement(0, 1)], true);
    let rgba =
        q0s_format::raster::rasterize_q0rg_frame(&project, 1, 0, 320, 200, 1, [255, 255, 255, 255]);
    let at = |x: usize, y: usize| {
        let i = (y * 320 + x) * 4;
        [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
    };
    assert_eq!(at(25, 35), [255, 0, 0, 255]);
    assert_eq!(at(5, 5), [255, 255, 255, 255]);
    let blue = at(50, 50);
    assert!(blue[0] > 110 && blue[0] < 145 && blue[2] == 255, "{blue:?}");
}
