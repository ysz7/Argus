//! The Argus benchmark suite: measures observations against ground truth
//! instead of judging them by demos.
//!
//! - [`dataset`]: recorded interface states (frame, accessibility tree,
//!   ground truth) and drafting the truth from an accessibility tree;
//! - [`run`]: replaying them through the real pipeline in several modes;
//! - [`metrics`]: recall, precision, role/text/grounding/state/relation
//!   accuracy, identity across steps, confidence calibration;
//! - [`report`]: summaries and the text report.
//!
//! Live latency and memory are measured by `argus benchmark latency`.

#![forbid(unsafe_code)]

pub mod dataset;
pub mod metrics;
mod replay;
pub mod report;
pub mod run;

pub use dataset::{Case, Step, Truth, TruthElement, draft_truth, load_case, load_dataset};
pub use report::{Report, render};
pub use run::{Mode, Options, run};

/// Result alias of the benchmark suite.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Why the benchmark cannot run.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The dataset is missing or malformed.
    #[error("dataset: {0}")]
    Dataset(String),
}
