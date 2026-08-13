//! Smoke tests for the drawing pipeline: simulate the model mutations a tool
//! performs (without going through egui events) to make sure asset id assignment,
//! placement insertion and dirty-flag propagation behave like a real session.

use q0s_format::v2::{
    Anchor, Asset, Path as VPath, Placement, Rgba, Stroke as VStroke, Target, Transform2D, Tween,
    Vec2, VectorAsset,
};

use q0editor::app::EditorApp;

fn commit_stroke(app: &mut EditorApp, anchors: Vec<Anchor>, closed: bool) -> u16 {
    let asset_id = app
        .state
        .project
        .assets
        .iter()
        .map(|a| a.id())
        .max()
        .unwrap_or(0)
        .saturating_add(1)
        .max(1);
    app.state.project.assets.push(Asset::Vector(VectorAsset {
        asset_id,
        paths: vec![VPath { anchors, closed }],
        fill: if closed {
            Some(Rgba {
                r: 200,
                g: 200,
                b: 200,
                a: 255,
            })
        } else {
            None
        },
        stroke: Some(VStroke {
            color: Rgba {
                r: 0,
                g: 0,
                b: 0,
                a: 255,
            },
            width: 1.0,
            cap: q0s_format::geom::CapShape::Round,
        }),
    }));
    let q0rg_id = app.session.current_q0rg_id;
    let layer_id = app.session.current_layer_id;
    let frame = app.session.current_frame;
    let q = app
        .state
        .project
        .q0rgs
        .iter_mut()
        .find(|q| q.q0rg_id == q0rg_id)
        .expect("q0rg");
    let layer = q
        .layers
        .iter_mut()
        .find(|l| l.layer_id == layer_id)
        .expect("layer");
    layer.placements.push(Placement {
        instance_id: 0,
        frame,
        target: Target::Asset(asset_id),
        transform: Transform2D::IDENTITY,
        tween: Tween::None,
        fx: Default::default(),
    });
    app.state.dirty = true;
    asset_id
}

#[test]
fn committing_a_pen_path_creates_asset_and_placement() {
    let mut app = EditorApp::default();
    assert!(app.state.project.assets.is_empty());
    assert!(app.state.project.q0rgs[0].layers[0].placements.is_empty());

    let anchors = vec![
        Anchor {
            point: Vec2::new(10.0, 10.0),
            in_handle: None,
            out_handle: None,
        },
        Anchor {
            point: Vec2::new(50.0, 10.0),
            in_handle: None,
            out_handle: None,
        },
        Anchor {
            point: Vec2::new(50.0, 50.0),
            in_handle: None,
            out_handle: None,
        },
    ];
    let asset_id = commit_stroke(&mut app, anchors, true);
    assert_eq!(asset_id, 1);
    assert_eq!(app.state.project.assets.len(), 1);
    let placements = &app.state.project.q0rgs[0].layers[0].placements;
    assert_eq!(placements.len(), 1);
    assert!(matches!(placements[0].target, Target::Asset(1)));
    assert!(app.state.dirty);
}

#[test]
fn second_commit_assigns_next_asset_id() {
    let mut app = EditorApp::default();
    let a = commit_stroke(
        &mut app,
        vec![
            Anchor {
                point: Vec2::new(0.0, 0.0),
                in_handle: None,
                out_handle: None,
            },
            Anchor {
                point: Vec2::new(10.0, 0.0),
                in_handle: None,
                out_handle: None,
            },
        ],
        false,
    );
    let b = commit_stroke(
        &mut app,
        vec![
            Anchor {
                point: Vec2::new(0.0, 0.0),
                in_handle: None,
                out_handle: None,
            },
            Anchor {
                point: Vec2::new(10.0, 10.0),
                in_handle: None,
                out_handle: None,
            },
        ],
        false,
    );
    assert_eq!(a, 1);
    assert_eq!(b, 2);
    assert_eq!(app.state.project.assets.len(), 2);
}

#[test]
fn brush_outline_traces_closed_polygon_around_centerline() {
    // Straight 2-point centerline → outline should have 2 (left+right) +
    // 2 cap segments. Cap_steps=8 → 7 intermediate cap points each side.
    let centerline = vec![Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0)];
    let outline = q0editor::tools::brush_outline(&centerline, 1.0, 8);
    assert!(outline.len() >= 4, "outline has only {} pts", outline.len());

    // The outline must be wide enough on both sides of the centerline:
    // at least one point ~ +1 in y, at least one ~ -1.
    let mut has_top = false;
    let mut has_bottom = false;
    for p in &outline {
        if (p.y - 1.0).abs() < 0.5 {
            has_top = true;
        }
        if (p.y + 1.0).abs() < 0.5 {
            has_bottom = true;
        }
    }
    assert!(
        has_top && has_bottom,
        "outline missing one side: {outline:?}"
    );

    // Bounding box should extend a bit past either end (round caps).
    let (min_x, max_x) = outline
        .iter()
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), p| {
            (lo.min(p.x), hi.max(p.x))
        });
    assert!(min_x < -0.1, "left cap should extend past x=0, got {min_x}");
    assert!(
        max_x > 10.1,
        "right cap should extend past x=10, got {max_x}"
    );
}

#[test]
fn brush_outline_returns_empty_for_degenerate_input() {
    assert!(q0editor::tools::brush_outline(&[], 1.0, 8).is_empty());
    assert!(q0editor::tools::brush_outline(&[Vec2::new(0.0, 0.0)], 1.0, 8).is_empty());
    assert!(
        q0editor::tools::brush_outline(&[Vec2::new(0.0, 0.0), Vec2::new(1.0, 0.0)], 0.0, 8)
            .is_empty()
    );
}

#[test]
fn convert_stroke_to_fill_creates_filled_asset() {
    use q0editor::app::Action;
    use q0editor::state::Selection;

    let mut app = EditorApp::default();
    let asset_id = commit_stroke(
        &mut app,
        vec![
            Anchor {
                point: Vec2::new(0.0, 0.0),
                in_handle: None,
                out_handle: None,
            },
            Anchor {
                point: Vec2::new(20.0, 0.0),
                in_handle: None,
                out_handle: None,
            },
        ],
        false,
    );
    assert_eq!(asset_id, 1);
    app.session.selection = Selection::Placement {
        q0rg_id: app.session.current_q0rg_id,
        layer_id: app.session.current_layer_id,
        placement_idx: 0,
    };

    app.queue(Action::ConvertStrokeToFill);
    app.flush_pending_actions();

    // Should have created a second asset that is filled (no stroke).
    assert_eq!(app.state.project.assets.len(), 2);
    let new_asset = app
        .state
        .project
        .assets
        .iter()
        .find(|a| a.id() != asset_id)
        .expect("new asset");
    if let Asset::Vector(v) = new_asset {
        assert!(v.fill.is_some(), "converted asset should be filled");
        assert!(v.stroke.is_none(), "converted asset should have no stroke");
        assert!(v.paths[0].closed, "fill path must be closed");
    } else {
        panic!("expected vector asset");
    }
    // Placement should now point at the new filled asset.
    let placement = &app.state.project.q0rgs[0].layers[0].placements[0];
    assert!(matches!(placement.target, Target::Asset(id) if id != asset_id));
}

#[test]
fn committed_project_validates_for_v2_writer() {
    let mut app = EditorApp::default();
    commit_stroke(
        &mut app,
        vec![
            Anchor {
                point: Vec2::new(5.0, 5.0),
                in_handle: None,
                out_handle: None,
            },
            Anchor {
                point: Vec2::new(15.0, 5.0),
                in_handle: None,
                out_handle: None,
            },
        ],
        false,
    );
    q0s_format::v2::validate(&app.state.project).expect("project validates");
    let bytes = q0s_format::v2::write(&app.state.project).expect("project writes");
    let parsed = q0s_format::v2::parse(&bytes).expect("project parses");
    assert_eq!(parsed.assets.len(), 1);
}

#[test]
fn deleting_selected_raw_contour_preserves_the_rest_of_the_drawing() {
    use q0editor::app::Action;
    use q0editor::state::Selection;

    let mut app = EditorApp::default();
    let asset_id = commit_stroke(
        &mut app,
        vec![
            Anchor {
                point: Vec2::new(0.0, 0.0),
                in_handle: None,
                out_handle: None,
            },
            Anchor {
                point: Vec2::new(20.0, 0.0),
                in_handle: None,
                out_handle: None,
            },
            Anchor {
                point: Vec2::new(20.0, 20.0),
                in_handle: None,
                out_handle: None,
            },
        ],
        true,
    );
    let Asset::Vector(vector) = app
        .state
        .project
        .assets
        .iter_mut()
        .find(|asset| asset.id() == asset_id)
        .expect("vector asset")
    else {
        unreachable!();
    };
    vector.paths.push(VPath {
        anchors: vec![
            Anchor {
                point: Vec2::new(100.0, 100.0),
                in_handle: None,
                out_handle: None,
            },
            Anchor {
                point: Vec2::new(120.0, 100.0),
                in_handle: None,
                out_handle: None,
            },
            Anchor {
                point: Vec2::new(120.0, 120.0),
                in_handle: None,
                out_handle: None,
            },
        ],
        closed: true,
    });

    app.session.selection = Selection::Path {
        q0rg_id: app.session.current_q0rg_id,
        layer_id: app.session.current_layer_id,
        placement_idx: 0,
        path_idx: 0,
    };
    app.queue(Action::DeleteSelection);
    app.flush_pending_actions();

    let Asset::Vector(vector) = &app.state.project.assets[0] else {
        unreachable!();
    };
    assert_eq!(vector.paths.len(), 1);
    assert_eq!(vector.paths[0].anchors[0].point, Vec2::new(100.0, 100.0));
    assert_eq!(app.state.project.q0rgs[0].layers[0].placements.len(), 1);
}
