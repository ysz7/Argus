use crate::{Bounds, Confidence, ElementState, Role, Source};

/// Identifier of a candidate, unique within the output of one source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CandidateId(pub u32);

/// Evidence for one element as reported by a single source, already mapped
/// to protocol vocabulary (roles, states, global coordinates).
///
/// Candidates are the common currency between sources (accessibility, OCR,
/// vision) and the stages that turn them into an
/// [`Observation`](crate::Observation). They are internal to Argus and not
/// part of the JSON protocol.
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
    /// Where the evidence came from.
    pub source: Source,
    /// Source-native role, kept for debugging (e.g. `AXButton`). Never
    /// exposed in observations.
    pub native_role: Option<String>,
}
