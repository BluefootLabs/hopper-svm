//! BPF compatibility surface backed by Agave.
//!
//! This module is **feature-gated** behind the `bpf-execution`
//! feature. Default Hopper SVM users get the built-in simulator
//! path. Callers that opt in keep the older registration API, but
//! loaded `.so` bytes execute through Agave's loader/runtime stack.
//!
//! ```toml
//! [dev-dependencies]
//! hopper-svm = { git = "https://github.com/BluefootLabs/hopper-svm", features = ["bpf-execution"] }
//! ```
//!
pub mod engine;

pub use engine::BpfEngine;
