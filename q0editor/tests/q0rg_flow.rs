use q0s_format::v2::{
    Anchor, Asset, Path as VPath, Placement, Stroke as VStroke, Target, Transform2D, Tween, Vec2,
    VectorAsset,
};

use q0editor::app::{Action, EditorApp};
use q0editor::state::Selection;

fn seed_simple_shape(app: &mut EditorApp) -> u16 {
    let asset_id = app
        .state
        .project
        .assets
        .iter()
        .map(|a| a.id())
        .max()
        .unwrap_or(0)
        + 1;
    app.state.project.assets.push(Asset::Vector(VectorAsset {
        asset_id,
        paths: vec![VPath {
            anchors: vec![
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
                Anchor {
                    point: Vec2::new(10.0, 10.0),
                    in_handle: None,
                    out_handle: None,
                },
            ],
            closed: true,
        }],
        fill: None,
        stroke: Some(VStroke {
            color: q0s_format::v2::Rgba {
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
    let q = app
        .state
        .project
        .q0rgs
        .iter_mut()
        .find(|q| q.q0rg_id == q0rg_id)
        .unwrap();
    let layer = q
        .layers
        .iter_mut()
        .find(|l| l.layer_id == layer_id)
        .unwrap();
    layer.placements.push(Placement {
        frame: 0,
        target: Target::Asset(asset_id),
        transform: Transform2D::IDENTITY,
        tween: Tween::None,
    });
    asset_id
}

#[test]
fn raw_graphics_backspace_shortcut_reaches_delete_action() {
    let mut app = EditorApp::default();
    seed_simple_shape(&mut app);
    let q0rg_id = app.session.current_q0rg_id;
    let layer_id = app.session.current_layer_id;
    app.session.selection = Selection::Path {
        q0rg_id,
        layer_id,
        placement_idx: 0,
        path_idx: 0,
    };

    invoke_shortcut(&mut app, egui::Key::Backspace, egui::Modifiers::NONE);

    let layer = app
        .state
        .project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        .unwrap()
        .layers
        .iter()
        .find(|layer| layer.layer_id == layer_id)
        .unwrap();
    assert!(layer.placements.is_empty());
}

#[test]
fn raw_graphics_ctrl_g_shortcut_reaches_convert_action() {
    let mut app = EditorApp::default();
    seed_simple_shape(&mut app);
    let q0rg_id = app.session.current_q0rg_id;
    let layer_id = app.session.current_layer_id;
    app.session.selection = Selection::Path {
        q0rg_id,
        layer_id,
        placement_idx: 0,
        path_idx: 0,
    };

    invoke_shortcut(&mut app, egui::Key::G, egui::Modifiers::COMMAND);

    let layer = app
        .state
        .project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        .unwrap()
        .layers
        .iter()
        .find(|layer| layer.layer_id == layer_id)
        .unwrap();
    assert!(matches!(
        layer.placements.as_slice(),
        [Placement {
            target: Target::Q0rg(_),
            ..
        }]
    ));
}

#[test]
fn convert_to_q0rg_replaces_placement_with_q0rg_instance() {
    let mut app = EditorApp::default();
    let original_asset = seed_simple_shape(&mut app);
    let q0rg_id = app.session.current_q0rg_id;
    let layer_id = app.session.current_layer_id;

    // Select the placement.
    app.session.selection = Selection::Placement {
        q0rg_id,
        layer_id,
        placement_idx: 0,
    };

    // Manually invoke convert (the action handler is the same code path).
    invoke_convert(&mut app);

    // The placement should now target a Q0rg, not the asset directly.
    let q = app
        .state
        .project
        .q0rgs
        .iter()
        .find(|q| q.q0rg_id == q0rg_id)
        .unwrap();
    let layer = q.layers.iter().find(|l| l.layer_id == layer_id).unwrap();
    let p = &layer.placements[0];
    let new_q0rg_id = match p.target {
        Target::Q0rg(id) => id,
        Target::Asset(_) => panic!("placement should now target a q0rg"),
    };

    // The new q0rg has a single placement targeting the original asset.
    let new_q = app
        .state
        .project
        .q0rgs
        .iter()
        .find(|q| q.q0rg_id == new_q0rg_id)
        .unwrap();
    assert_eq!(new_q.layers.len(), 1);
    assert_eq!(new_q.layers[0].placements.len(), 1);
    assert!(matches!(
        new_q.layers[0].placements[0].target,
        Target::Asset(id) if id == original_asset
    ));

    // Project still validates as v2.
    q0s_format::v2::validate(&app.state.project).expect("validates");
}

#[test]
fn convert_to_q0rg_keeps_vector_appearance_attached_to_the_asset() {
    let mut app = EditorApp::default();
    let asset_id = seed_simple_shape(&mut app);
    let Asset::Vector(vector) = app
        .state
        .project
        .assets
        .iter_mut()
        .find(|asset| asset.id() == asset_id)
        .expect("seed asset")
    else {
        unreachable!();
    };
    vector.fill = Some(q0s_format::v2::Rgba {
        r: 220,
        g: 50,
        b: 30,
        a: 255,
    });
    vector.stroke = None;
    let appearance = q0s_format::v2::VectorAppearance {
        material: q0s_format::v2::VectorMaterial::SoftHalo {
            radius: 7.0,
            opacity: 0.6,
        },
        erase_mask: vec![VPath {
            anchors: vec![
                Anchor {
                    point: Vec2::new(2.0, 2.0),
                    in_handle: None,
                    out_handle: None,
                },
                Anchor {
                    point: Vec2::new(5.0, 2.0),
                    in_handle: None,
                    out_handle: None,
                },
                Anchor {
                    point: Vec2::new(5.0, 5.0),
                    in_handle: None,
                    out_handle: None,
                },
            ],
            closed: true,
        }],
        material_source: Vec::new(),
        clip_mask: Vec::new(),
    };
    app.state
        .project
        .asset_appearances
        .insert(asset_id, appearance.clone());
    let q0rg_id = app.session.current_q0rg_id;
    let layer_id = app.session.current_layer_id;
    app.session.selection = Selection::Placement {
        q0rg_id,
        layer_id,
        placement_idx: 0,
    };

    invoke_convert(&mut app);

    assert_eq!(app.state.project.asset_appearances[&asset_id], appearance);
    let outer = app
        .state
        .project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        .expect("stage q0rg");
    let Target::Q0rg(inner_id) = outer.layers[0].placements[0].target else {
        panic!("converted placement must target q0rg");
    };
    let inner = app
        .state
        .project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == inner_id)
        .expect("converted q0rg");
    assert!(matches!(
        inner.layers[0].placements[0].target,
        Target::Asset(id) if id == asset_id
    ));
    q0s_format::v2::validate(&app.state.project).expect("appearance survives valid conversion");
}

#[test]
fn convert_to_q0rg_packs_marquee_multi_selection_into_one_symbol() {
    use q0editor::state::PlacementRef;

    let mut app = EditorApp::default();
    // Two distinct shapes on the same layer.
    let asset_a = seed_simple_shape(&mut app);
    let asset_b = seed_simple_shape(&mut app);
    let q0rg_id = app.session.current_q0rg_id;
    let layer_id = app.session.current_layer_id;
    // Sanity: both placements live on the layer now.
    let layer = app
        .state
        .project
        .q0rgs
        .iter()
        .find(|q| q.q0rg_id == q0rg_id)
        .unwrap()
        .layers
        .iter()
        .find(|l| l.layer_id == layer_id)
        .unwrap();
    assert_eq!(layer.placements.len(), 2);

    // Marquee both.
    app.session.selection = Selection::Multi(vec![
        PlacementRef {
            q0rg_id,
            layer_id,
            placement_idx: 0,
        },
        PlacementRef {
            q0rg_id,
            layer_id,
            placement_idx: 1,
        },
    ]);

    invoke_convert(&mut app);

    // The original layer should now have exactly one placement: the new
    // q0rg instance.
    let q = app
        .state
        .project
        .q0rgs
        .iter()
        .find(|q| q.q0rg_id == q0rg_id)
        .unwrap();
    let layer = q.layers.iter().find(|l| l.layer_id == layer_id).unwrap();
    assert_eq!(
        layer.placements.len(),
        1,
        "outer layer should hold one Q0rg instance after convert"
    );
    let new_q0rg_id = match layer.placements[0].target {
        Target::Q0rg(id) => id,
        Target::Asset(_) => panic!("outer placement should target a q0rg"),
    };

    // Inner q0rg has both original assets, frame=0, transforms preserved.
    let inner = app
        .state
        .project
        .q0rgs
        .iter()
        .find(|q| q.q0rg_id == new_q0rg_id)
        .unwrap();
    let inner_layer = inner.layers.first().expect("inner layer");
    assert_eq!(inner_layer.placements.len(), 2);
    let mut targets: Vec<u16> = inner_layer
        .placements
        .iter()
        .filter_map(|p| match p.target {
            Target::Asset(id) => Some(id),
            _ => None,
        })
        .collect();
    targets.sort_unstable();
    assert_eq!(targets, vec![asset_a, asset_b]);
    for p in &inner_layer.placements {
        assert_eq!(p.frame, 0);
    }

    // And the project still validates.
    q0s_format::v2::validate(&app.state.project).expect("validates");
}

#[test]
fn place_q0rg_instance_adds_placement_at_stage_center() {
    let mut app = EditorApp::default();
    // Add a sibling q0rg.
    let new_q0rg_id = app
        .state
        .project
        .q0rgs
        .iter()
        .map(|q| q.q0rg_id)
        .max()
        .unwrap()
        + 1;
    app.state.project.q0rgs.push(q0s_format::v2::Q0rg {
        q0rg_id: new_q0rg_id,
        name: "Sibling".to_string(),
        frame_count: 1,
        script: String::new(),
        layers: vec![q0s_format::v2::Layer {
            layer_id: 1,
            name: "Layer 1".to_string(),
            explicit_keyframes: Vec::new(),
            placements: Vec::new(),
        }],
    });

    invoke_place_instance(&mut app, new_q0rg_id);

    let q = app
        .state
        .project
        .q0rgs
        .iter()
        .find(|q| q.q0rg_id == app.session.current_q0rg_id)
        .unwrap();
    let layer = q
        .layers
        .iter()
        .find(|l| l.layer_id == app.session.current_layer_id)
        .unwrap();
    assert_eq!(layer.placements.len(), 1);
    assert!(matches!(layer.placements[0].target, Target::Q0rg(id) if id == new_q0rg_id));
    let t = layer.placements[0].transform;
    assert_eq!(t.tx, app.state.project.meta.stage_width as f32 / 2.0);
    assert_eq!(t.ty, app.state.project.meta.stage_height as f32 / 2.0);
}

#[test]
fn place_q0rg_instance_rejects_indirect_cycle() {
    let mut app = EditorApp::default();
    let root_id = app.session.current_q0rg_id;
    let child_id = root_id + 1;
    app.state.project.q0rgs.push(q0s_format::v2::Q0rg {
        q0rg_id: child_id,
        name: "Child".to_string(),
        frame_count: 1,
        script: String::new(),
        layers: vec![q0s_format::v2::Layer {
            layer_id: 1,
            name: "Layer 1".to_string(),
            explicit_keyframes: Vec::new(),
            placements: vec![Placement {
                frame: 0,
                target: Target::Q0rg(root_id),
                transform: Transform2D::IDENTITY,
                tween: Tween::None,
            }],
        }],
    });

    invoke_place_instance(&mut app, child_id);

    let root = app
        .state
        .project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == root_id)
        .unwrap();
    assert!(root.layers.iter().all(|layer| layer.placements.is_empty()));
    q0s_format::v2::validate(&app.state.project).expect("project remains valid");
}

#[test]
fn enter_then_breadcrumb_jump_returns_to_root() {
    let mut app = EditorApp::default();
    let stage_id = app.session.current_q0rg_id;

    // Add a sibling and enter it.
    let child_id = app
        .state
        .project
        .q0rgs
        .iter()
        .map(|q| q.q0rg_id)
        .max()
        .unwrap()
        + 1;
    app.state.project.q0rgs.push(q0s_format::v2::Q0rg {
        q0rg_id: child_id,
        name: "Child".to_string(),
        frame_count: 1,
        script: String::new(),
        layers: vec![q0s_format::v2::Layer {
            layer_id: 1,
            name: "Layer 1".to_string(),
            explicit_keyframes: Vec::new(),
            placements: Vec::new(),
        }],
    });

    invoke_enter(&mut app, child_id);
    assert_eq!(app.session.current_q0rg_id, child_id);
    assert_eq!(app.session.breadcrumb, vec![stage_id]);

    invoke_breadcrumb_jump(&mut app, 0);
    assert_eq!(app.session.current_q0rg_id, stage_id);
    assert!(app.session.breadcrumb.is_empty());
}

#[test]
fn convert_selected_raw_fill_to_q0rg_preserves_neighbouring_graphics() {
    use q0editor::state::PathRef;

    let square = |x: f32| VPath {
        anchors: vec![
            Anchor {
                point: Vec2::new(x, 0.0),
                in_handle: None,
                out_handle: None,
            },
            Anchor {
                point: Vec2::new(x + 20.0, 0.0),
                in_handle: None,
                out_handle: None,
            },
            Anchor {
                point: Vec2::new(x + 20.0, 20.0),
                in_handle: None,
                out_handle: None,
            },
            Anchor {
                point: Vec2::new(x, 20.0),
                in_handle: None,
                out_handle: None,
            },
        ],
        closed: true,
    };
    let mut app = EditorApp::default();
    app.state.project.assets.push(Asset::Vector(VectorAsset {
        asset_id: 1,
        paths: vec![square(0.0), square(100.0)],
        fill: Some(q0s_format::v2::Rgba {
            r: 0,
            g: 0,
            b: 0,
            a: 255,
        }),
        stroke: None,
    }));
    app.state.project.q0rgs[0].layers[0]
        .placements
        .push(Placement {
            frame: 0,
            target: Target::Asset(1),
            transform: Transform2D::IDENTITY,
            tween: Tween::None,
        });
    app.session.selection = Selection::Paths(vec![PathRef {
        q0rg_id: 1,
        layer_id: 1,
        placement_idx: 0,
        path_idx: 0,
    }]);

    invoke_convert(&mut app);

    let outer = &app.state.project.q0rgs[0].layers[0].placements;
    assert_eq!(
        outer.len(),
        2,
        "neighbouring raw graphics must remain beside the symbol"
    );
    let symbol_id = outer
        .iter()
        .find_map(|placement| match placement.target {
            Target::Q0rg(id) => Some(id),
            Target::Asset(_) => None,
        })
        .expect("converted raw selection becomes a q0rg instance");
    let remainder_asset = outer
        .iter()
        .find_map(|placement| match placement.target {
            Target::Asset(id) => Some(id),
            Target::Q0rg(_) => None,
        })
        .expect("unselected raw neighbour remains on stage");
    let remainder = app
        .state
        .project
        .assets
        .iter()
        .find_map(|asset| match asset {
            Asset::Vector(vector) if vector.asset_id == remainder_asset => Some(vector),
            _ => None,
        })
        .unwrap();
    assert_eq!(remainder.paths.len(), 1);
    assert!(remainder.paths[0].anchors[0].point.x >= 100.0);

    let symbol = app
        .state
        .project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == symbol_id)
        .unwrap();
    assert_eq!(symbol.layers[0].placements.len(), 1);
    let selected_asset = match symbol.layers[0].placements[0].target {
        Target::Asset(id) => id,
        Target::Q0rg(_) => panic!("raw artwork inside the symbol must remain vector data"),
    };
    let selected = app
        .state
        .project
        .assets
        .iter()
        .find_map(|asset| match asset {
            Asset::Vector(vector) if vector.asset_id == selected_asset => Some(vector),
            _ => None,
        })
        .unwrap();
    assert_eq!(selected.paths.len(), 1);
    assert!(selected.paths[0].anchors[0].point.x < 50.0);
    q0s_format::v2::validate(&app.state.project).expect("raw conversion keeps project valid");
}

#[test]
fn delete_action_removes_selected_raw_fill_but_keeps_its_neighbour() {
    use q0editor::state::PathRef;

    let square = |x: f32| VPath {
        anchors: vec![
            Anchor {
                point: Vec2::new(x, 0.0),
                in_handle: None,
                out_handle: None,
            },
            Anchor {
                point: Vec2::new(x + 20.0, 0.0),
                in_handle: None,
                out_handle: None,
            },
            Anchor {
                point: Vec2::new(x + 20.0, 20.0),
                in_handle: None,
                out_handle: None,
            },
            Anchor {
                point: Vec2::new(x, 20.0),
                in_handle: None,
                out_handle: None,
            },
        ],
        closed: true,
    };
    let mut app = EditorApp::default();
    app.state.project.assets.push(Asset::Vector(VectorAsset {
        asset_id: 1,
        paths: vec![square(0.0), square(100.0)],
        fill: Some(q0s_format::v2::Rgba {
            r: 0,
            g: 0,
            b: 0,
            a: 255,
        }),
        stroke: None,
    }));
    app.state.project.q0rgs[0].layers[0]
        .placements
        .push(Placement {
            frame: 0,
            target: Target::Asset(1),
            transform: Transform2D::IDENTITY,
            tween: Tween::None,
        });
    app.session.selection = Selection::Paths(vec![PathRef {
        q0rg_id: 1,
        layer_id: 1,
        placement_idx: 0,
        path_idx: 0,
    }]);

    drive_one(&mut app, Action::DeleteSelection);

    let vector = app
        .state
        .project
        .assets
        .iter()
        .find_map(|asset| match asset {
            Asset::Vector(vector) if vector.asset_id == 1 => Some(vector),
            _ => None,
        })
        .unwrap();
    assert_eq!(vector.paths.len(), 1);
    assert!(vector.paths[0].anchors[0].point.x >= 100.0);
    assert!(matches!(app.session.selection, Selection::None));
}

fn seed_mixed_raw_area(app: &mut EditorApp) -> (u16, u16, usize, usize) {
    let raw_asset_id = app
        .state
        .project
        .assets
        .iter()
        .map(|asset| asset.id())
        .max()
        .unwrap_or(0)
        + 1;
    app.state.project.assets.push(Asset::Vector(VectorAsset {
        asset_id: raw_asset_id,
        paths: vec![VPath {
            anchors: vec![
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
                Anchor {
                    point: Vec2::new(0.0, 20.0),
                    in_handle: None,
                    out_handle: None,
                },
            ],
            closed: true,
        }],
        fill: Some(q0s_format::v2::Rgba {
            r: 10,
            g: 20,
            b: 30,
            a: 255,
        }),
        stroke: None,
    }));

    let child_q0rg_id = app
        .state
        .project
        .q0rgs
        .iter()
        .map(|q0rg| q0rg.q0rg_id)
        .max()
        .unwrap_or(0)
        + 1;
    app.state.project.q0rgs.push(q0s_format::v2::Q0rg {
        q0rg_id: child_q0rg_id,
        name: "Child".to_string(),
        frame_count: 1,
        script: String::new(),
        layers: vec![q0s_format::v2::Layer {
            layer_id: 1,
            name: "Layer 1".to_string(),
            explicit_keyframes: Vec::new(),
            placements: vec![Placement {
                frame: 0,
                target: Target::Asset(raw_asset_id),
                transform: Transform2D::IDENTITY,
                tween: Tween::None,
            }],
        }],
    });

    let q0rg_id = app.session.current_q0rg_id;
    let layer_id = app.session.current_layer_id;
    let layer = app
        .state
        .project
        .q0rgs
        .iter_mut()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        .unwrap()
        .layers
        .iter_mut()
        .find(|layer| layer.layer_id == layer_id)
        .unwrap();
    let raw_idx = layer.placements.len();
    layer.placements.push(Placement {
        frame: 0,
        target: Target::Asset(raw_asset_id),
        transform: Transform2D::IDENTITY,
        tween: Tween::None,
    });
    let object_idx = layer.placements.len();
    layer.placements.push(Placement {
        frame: 0,
        target: Target::Q0rg(child_q0rg_id),
        transform: Transform2D {
            tx: 5.0,
            ty: 0.0,
            ..Transform2D::IDENTITY
        },
        tween: Tween::None,
    });
    (q0rg_id, layer_id, raw_idx, object_idx)
}

fn select_mixed_raw_area(
    app: &mut EditorApp,
    q0rg_id: u16,
    layer_id: u16,
    raw_idx: usize,
    object_idx: usize,
) {
    app.session.selection = Selection::RawArea {
        placements: vec![q0editor::state::PlacementRef {
            q0rg_id,
            layer_id,
            placement_idx: raw_idx,
        }],
        objects: vec![q0editor::state::PlacementRef {
            q0rg_id,
            layer_id,
            placement_idx: object_idx,
        }],
        bounds_min: Vec2::new(0.0, 0.0),
        bounds_max: Vec2::new(10.0, 10.0),
    };
}

#[test]
fn convert_dotted_mixed_selection_to_q0rg() {
    let mut app = EditorApp::default();
    let (q0rg_id, layer_id, raw_idx, object_idx) = seed_mixed_raw_area(&mut app);
    select_mixed_raw_area(&mut app, q0rg_id, layer_id, raw_idx, object_idx);

    invoke_convert(&mut app);

    let Selection::Placement { placement_idx, .. } = app.session.selection else {
        panic!("converted mixed selection should become one q0rg placement");
    };
    let root = app
        .state
        .project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        .unwrap();
    let layer = root
        .layers
        .iter()
        .find(|layer| layer.layer_id == layer_id)
        .unwrap();
    assert_eq!(layer.placements.len(), 2, "raw remainder + new symbol");
    let new_q0rg_id = match layer.placements[placement_idx].target {
        Target::Q0rg(id) => id,
        _ => panic!("outer placement must target new q0rg"),
    };
    let inner = app
        .state
        .project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == new_q0rg_id)
        .unwrap();
    assert_eq!(inner.layers[0].placements.len(), 2);
    assert!(inner.layers[0]
        .placements
        .iter()
        .any(|placement| matches!(placement.target, Target::Asset(_))));
    assert!(inner.layers[0]
        .placements
        .iter()
        .any(|placement| matches!(placement.target, Target::Q0rg(_))));
    q0s_format::v2::validate(&app.state.project).expect("mixed symbol project validates");
}

#[test]
fn edit_actions_copy_paste_duplicate_mixed_raw_area() {
    let mut app = EditorApp::default();
    let (q0rg_id, layer_id, raw_idx, object_idx) = seed_mixed_raw_area(&mut app);
    select_mixed_raw_area(&mut app, q0rg_id, layer_id, raw_idx, object_idx);

    drive_one(&mut app, Action::CopySelection);
    let clipboard = app.session.clipboard.as_ref().expect("clipboard");
    assert_eq!(clipboard.raw_vectors.len(), 1);
    assert_eq!(clipboard.placements.len(), 1);

    drive_one(&mut app, Action::Paste);
    let count_after_paste = app.state.project.q0rgs[0].layers[0].placements.len();
    assert_eq!(count_after_paste, 4);
    assert!(matches!(app.session.selection, Selection::RawArea { .. }));

    drive_one(&mut app, Action::DuplicateSelection);
    assert_eq!(app.state.project.q0rgs[0].layers[0].placements.len(), 6);
    q0s_format::v2::validate(&app.state.project).expect("copy/paste/duplicate validates");
}

#[test]
fn mixed_raw_area_edit_shortcuts_reach_copy_duplicate_cut_and_paste() {
    let mut app = EditorApp::default();
    let (q0rg_id, layer_id, raw_idx, object_idx) = seed_mixed_raw_area(&mut app);
    select_mixed_raw_area(&mut app, q0rg_id, layer_id, raw_idx, object_idx);

    invoke_shortcut(&mut app, egui::Key::C, egui::Modifiers::COMMAND);
    let clipboard = app
        .session
        .clipboard
        .as_ref()
        .expect("copy shortcut clipboard");
    assert_eq!(clipboard.raw_vectors.len(), 1);
    assert_eq!(clipboard.placements.len(), 1);

    invoke_shortcut(&mut app, egui::Key::D, egui::Modifiers::COMMAND);
    assert_eq!(app.state.project.q0rgs[0].layers[0].placements.len(), 4);
    assert!(matches!(app.session.selection, Selection::RawArea { .. }));

    invoke_shortcut(&mut app, egui::Key::X, egui::Modifiers::COMMAND);
    assert_eq!(app.state.project.q0rgs[0].layers[0].placements.len(), 2);
    assert!(matches!(app.session.selection, Selection::None));

    invoke_shortcut(&mut app, egui::Key::V, egui::Modifiers::COMMAND);
    assert_eq!(app.state.project.q0rgs[0].layers[0].placements.len(), 4);
    assert!(matches!(app.session.selection, Selection::RawArea { .. }));
    q0s_format::v2::validate(&app.state.project).expect("shortcut edit sequence validates");
}

#[test]
fn convert_dotted_raw_area_without_objects_to_q0rg() {
    let mut app = EditorApp::default();
    let (q0rg_id, layer_id, raw_idx, _object_idx) = seed_mixed_raw_area(&mut app);
    app.state.project.q0rgs[0].layers[0].placements.pop();
    app.session.selection = Selection::RawArea {
        placements: vec![q0editor::state::PlacementRef {
            q0rg_id,
            layer_id,
            placement_idx: raw_idx,
        }],
        objects: Vec::new(),
        bounds_min: Vec2::new(0.0, 0.0),
        bounds_max: Vec2::new(10.0, 10.0),
    };

    invoke_convert(&mut app);

    assert!(matches!(app.session.selection, Selection::Placement { .. }));
    let root = app
        .state
        .project
        .q0rgs
        .iter()
        .find(|q0rg| q0rg.q0rg_id == q0rg_id)
        .unwrap();
    assert!(root.layers[0]
        .placements
        .iter()
        .any(|placement| matches!(placement.target, Target::Q0rg(_))));
    q0s_format::v2::validate(&app.state.project).expect("dotted raw-area symbol validates");
}

#[test]
fn edit_cut_removes_mixed_selection_and_paste_restores_both_kinds() {
    let mut app = EditorApp::default();
    let (q0rg_id, layer_id, raw_idx, object_idx) = seed_mixed_raw_area(&mut app);
    select_mixed_raw_area(&mut app, q0rg_id, layer_id, raw_idx, object_idx);

    drive_one(&mut app, Action::CutSelection);
    let root_layer = &app.state.project.q0rgs[0].layers[0];
    assert_eq!(
        root_layer.placements.len(),
        1,
        "only raw remainder stays after cut"
    );
    assert!(app.session.clipboard.is_some());

    drive_one(&mut app, Action::Paste);
    let root_layer = &app.state.project.q0rgs[0].layers[0];
    assert_eq!(
        root_layer.placements.len(),
        3,
        "remainder + pasted raw + pasted q0rg"
    );
    assert!(root_layer
        .placements
        .iter()
        .any(|placement| matches!(placement.target, Target::Q0rg(_))));
    q0s_format::v2::validate(&app.state.project).expect("cut/paste validates");
}

// ---- internal: dispatch actions through the public action queue ----

fn invoke_convert(app: &mut EditorApp) {
    drive_one(app, Action::ConvertSelectionToQ0rg);
}

fn invoke_place_instance(app: &mut EditorApp, q0rg_id: u16) {
    drive_one(app, Action::PlaceQ0rgInstance(q0rg_id));
}

fn invoke_enter(app: &mut EditorApp, q0rg_id: u16) {
    drive_one(app, Action::EnterQ0rg(q0rg_id));
}

fn invoke_breadcrumb_jump(app: &mut EditorApp, depth: usize) {
    drive_one(app, Action::BreadcrumbJumpTo(depth));
}

fn invoke_shortcut(app: &mut EditorApp, key: egui::Key, modifiers: egui::Modifiers) {
    let ctx = egui::Context::default();
    ctx.begin_frame(egui::RawInput {
        events: vec![egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }],
        ..Default::default()
    });
    q0editor::panels::menu::handle_global_shortcuts(app, &ctx);
    let _ = ctx.end_frame();
    app.flush_pending_actions();
}

/// Drive a single Action through the editor without spinning up an egui Context.
/// We rely on the fact that none of the q0rg actions need viewport commands.
fn drive_one(app: &mut EditorApp, action: Action) {
    app.queue(action);
    app.flush_pending_actions();
}
