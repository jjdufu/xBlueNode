//! `xBlueStudio` modeling document, validation, diff, and multi-subgraph merge domain module (`06-STEP05-06` §05).

pub mod diff;
pub mod doc;
pub mod merge;
pub mod validate;

pub use diff::*;
pub use doc::*;
pub use merge::*;
pub use validate::*;
