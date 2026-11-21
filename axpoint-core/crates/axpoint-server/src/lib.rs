//! Library surface exposed by `axpoint-server` for offline evaluation harnesses (`sim_ask_eval`).
#![doc(test(attr(deny(warnings), forbid(unsafe_code))))]

pub mod ops;
pub mod policy;
pub mod sim_ask;
pub mod view;
