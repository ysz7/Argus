//! Element candidates: the common currency between sources (accessibility,
//! OCR, vision) and the stages that turn them into an
//! [`Observation`](crate::Observation).
//!
//! ```text
//! source ──SourceCandidate──> normalization ──ElementCandidate──> assembly
//! ```
//!
//! Candidates are internal to Argus and not part of the JSON protocol.

use crate::{Bounds, Confidence, ElementState, FrameGeometry, RelationKind, Role, Score, Source};

/// Identifier of a candidate, unique within the output of one source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CandidateId(pub u32);

/// Raw evidence for one element as reported by a single source.
///
/// A source maps its native vocabulary to protocol roles and states, because
/// only the source knows that vocabulary. Everything generic (coordinates,
/// text cleanup, visibility, consistency of states and confidence) is left to
/// normalization.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceCandidate {
    /// Identifier within the source's output.
    pub id: CandidateId,
    /// Structural parent reported by the same source.
    pub parent: Option<CandidateId>,
    /// Role hypothesis.
    pub role: Role,
    /// Name or recognized text, uncleaned.
    pub name: Option<String>,
    /// Value, uncleaned.
    pub value: Option<String>,
    /// Longer description, uncleaned.
    pub description: Option<String>,
    /// Where the element is, in the source's coordinate space.
    pub region: Region,
    /// Visible area of the container the element lives in, in global screen
    /// points. `None` if the source cannot tell.
    pub clip: Option<Bounds>,
    /// State hypotheses.
    pub state: ElementState,
    /// Confidence per property, as assessed by the source.
    pub confidence: Confidence,
    /// Relations to other candidates of the same source.
    pub relations: Vec<CandidateRelation>,
    /// Source identity and native metadata.
    pub meta: SourceMeta,
}

/// A location in one of the coordinate spaces sources work in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Region {
    /// Global screen points (accessibility APIs report these).
    Screen(Bounds),
    /// A rectangle in the pixels of a captured frame (OCR, vision).
    Frame {
        /// Placement of the frame the rectangle belongs to.
        geometry: FrameGeometry,
        /// The rectangle, in frame pixels.
        rect: PixelRect,
    },
}

/// A rectangle in frame pixels. Values may be fractional (sub-pixel boxes).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PixelRect {
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// Width.
    pub width: f32,
    /// Height.
    pub height: f32,
}

/// Identity of the evidence within its source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceMeta {
    /// The source.
    pub source: Source,
    /// Native role for debugging (e.g. `AXButton/AXCloseButton`). Never
    /// exposed in observations.
    pub native_role: Option<String>,
    /// Identifier assigned by the application or platform, if any (e.g.
    /// `AXIdentifier`). Useful for tracking; never exposed in observations.
    pub native_id: Option<String>,
}

impl SourceMeta {
    /// Metadata with only the source set.
    pub fn new(source: Source) -> Self {
        Self { source, native_role: None, native_id: None }
    }
}

/// A relation from a candidate to another candidate of the same source.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CandidateRelation {
    /// Relation type (`self <kind> target`).
    pub kind: RelationKind,
    /// The other candidate.
    pub target: CandidateId,
    /// Confidence that the relation holds.
    pub confidence: Option<Score>,
}

/// Normalized evidence for one element: global coordinates, cleaned text,
/// consistent states and confidence.
///
/// Produced only by normalization; assembled into observations.
#[derive(Debug, Clone, PartialEq)]
pub struct ElementCandidate {
    /// Identifier within the source's output.
    pub id: CandidateId,
    /// Structural parent reported by the same source.
    pub parent: Option<CandidateId>,
    /// Role hypothesis.
    pub role: Role,
    /// Name or visible label.
    pub name: Option<String>,
    /// Current value.
    pub value: Option<String>,
    /// Longer description or help text.
    pub description: Option<String>,
    /// Full extent in global screen points.
    pub bounds: Bounds,
    /// Visible part of `bounds`, when only partially visible.
    pub visible_bounds: Option<Bounds>,
    /// State hypotheses.
    pub state: ElementState,
    /// Confidence per property.
    pub confidence: Confidence,
    /// Relations to other candidates of the same source.
    pub relations: Vec<CandidateRelation>,
    /// Source identity and native metadata.
    pub meta: SourceMeta,
}
