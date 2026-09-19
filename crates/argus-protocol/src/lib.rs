//! Argus Observation Protocol.
//!
//! This crate defines the public, platform-independent language Argus uses to
//! describe a user interface: observations, elements, roles, bounds, states,
//! confidence, sources and relations. The normative description lives in
//! `spec/ARGUS_PROTOCOL.md`.
//!
//! Boundaries:
//! - depends on no other Argus crate;
//! - contains no platform code, no I/O and no perception logic;
//! - every type here is part of the serialization contract and must change
//!   deliberately.
//!
//! Invariants of single values (confidence range, finite bounds, non-empty
//! identifiers) are enforced on construction and deserialization, so an
//! invalid value cannot exist. Referential invariants of a whole observation
//! (unique IDs, consistent hierarchy) are checked by
//! [`Observation::validate`].
//!
//! [`Frame`] is the in-memory input of pixel-based perception. It is shared by
//! capture and perception crates but is not serialized as part of the JSON
//! protocol.

#![forbid(unsafe_code)]

mod candidate;
mod confidence;
mod delta;
mod element;
mod error;
mod frame;
mod geometry;
mod ids;
mod observation;
mod relation;
mod source;
mod validation;

pub use candidate::{
    CandidateId, CandidateRelation, ElementCandidate, PixelRect, Region, SourceCandidate,
    SourceMeta,
};
pub use confidence::{Confidence, Score};
pub use delta::{DeltaError, ElementChange, ObservationDelta};
pub use element::{CheckState, Element, ElementState, Role, TextRange, TextRun, TextState};
pub use error::{Error, Result};
pub use frame::{Frame, FrameGeometry, FrameId, PixelBuffer};
pub use geometry::Bounds;
pub use ids::{ElementId, ObservationId};
pub use observation::{Application, Observation, Timestamp, Window};
pub use relation::{Relation, RelationKind};
pub use source::Source;
pub use validation::ValidationError;

/// Version of the Argus Observation Protocol implemented by this crate.
pub const PROTOCOL_VERSION: &str = "0.2";
