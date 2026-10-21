//! Simulation shared by q0editor, which bakes it into keyframes, and
//! q0player, which runs it live for q0lang.
//!
//! Everything works in stage pixels and seconds, with y pointing down and
//! angles turning clockwise.

pub mod contour;
pub mod links;
pub mod liquid;
pub mod particles;
pub mod rigid;
mod rng;

pub use rigid::{
    attachment_point, body_collider, convex_hull, fit_bounds, posed_transform, BodyPose, BodyStart,
    ColliderGeometry, Fit, LinkStart, World,
};
