//! The 3D engine: models with bones and animation clips, a camera and 3D
//! lights per timeline, and drawings placed in 3D as planes.
//!
//! [`model`] holds what is stored, [`scene`] how it sits on a timeline,
//! [`draw`] turns a frame into something to draw, [`soft`] draws it in
//! software and [`frame`] gathers a timeline's 3D for one frame.

pub mod draw;
pub mod frame;
pub mod liquid;
pub mod math;
pub mod model;
pub mod scene;
pub mod soft;
pub mod wire;

pub use draw::{DrawItem, Frame3d, Image, MeshDraw, PlaneDraw, View3d};
pub use math::{Mat4, Quat, V3};
pub use model::{
    Channel, ChannelTarget, Clip, Face, Material, Mesh, ModelAsset, ModelData, Node, Outline,
    RenderMesh, Shading, Skin, Texture, Trs,
};
pub use scene::{
    Camera3d, CameraPose, Light3d, Light3dKind, LightRig, PoseTrack, Scene3dData, Scene3dSetup,
    Space3d, NO_CLIP,
};

/// The most 3D lights a frame shines with.
pub const MAX_LIGHTS_3D: usize = 16;
