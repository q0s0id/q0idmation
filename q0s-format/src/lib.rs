pub mod error;
pub mod geom;
mod io;
pub mod migrate;
pub mod model;
pub mod parser;
pub mod q0lang;
pub mod q0s_v2;
pub mod q0s_writer;
pub mod q1s;
pub mod raster;
pub mod rig;
pub mod transform;
pub mod v2;

pub use error::Error;
pub use migrate::migrate_v1_to_v2;
pub use model::{Background, Bitmap, Header, Movie, Placement, MAGIC, SUPPORTED_VERSION};
pub use parser::parse_q0s;
pub use q0s_v2::{
    is_q0s_v2, parse_q0s_v2, write_q0s_v2, Q0S_V2_MAGIC, Q0S_V2_VERSION, Q0S_VERSION_CURRENT,
};
pub use q0s_writer::write_q0s;
pub use q1s::{
    parse_q1s, q1s_to_movie, q1s_to_q0s_bytes, validate_q1s, write_q1s, Q1AssetBitmap, Q1Layer,
    Q1Placement, Q1Project, Q1ProjectMeta, Q1Scene, Q1S_MAGIC, Q1S_VERSION,
};
pub use v2::MAX_Q0RG_NESTING_DEPTH;
