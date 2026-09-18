//! What fusion knows about how each element came to be.

use std::fmt;

use argus_protocol::{Bounds, Confidence, ElementState, Role, Source};
use serde::Serialize;

/// Evidence behind one fused element.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ElementEvidence {
    /// Every source candidate that contributed, the creating one first.
    pub contributions: Vec<Contribution>,
    /// Disagreements between the contributions.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub conflicts: Vec<Conflict>,
}

/// Evidence from one source candidate.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Contribution {
    /// The source.
    pub source: Source,
    /// How the candidate relates to the element.
    pub link: Link,
    /// The candidate's role hypothesis.
    pub role: Role,
    /// The candidate's name (for OCR: the recognized text).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The candidate's value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// The candidate's state hypotheses.
    #[serde(skip_serializing_if = "ElementState::is_unknown")]
    pub state: ElementState,
    /// The candidate's bounds.
    pub bounds: Bounds,
    /// The candidate's confidence, as reported by its source.
    pub confidence: Confidence,
    /// Native role, for debugging.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_role: Option<String>,
}

/// How a contribution relates to the element.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Link {
    /// The element was created from this candidate.
    Primary,
    /// The candidate describes the same object.
    Same,
    /// Text recognized on the element: its visible label or value.
    Text,
    /// A detail drawn inside the element (e.g. a glyph), not a separate object.
    Part,
    /// A text that labels the element (it names an unnamed element).
    Label,
}

impl fmt::Display for Link {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Link::Primary => "primary",
            Link::Same => "same",
            Link::Text => "text",
            Link::Part => "part",
            Link::Label => "label",
        })
    }
}

/// A property sources can disagree about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Property {
    /// `role`.
    Role,
    /// `name`.
    Name,
    /// `value`.
    Value,
    /// Text visible on a control, compared with its accessible name. A
    /// difference is normal (icon buttons) and does not lower confidence.
    VisibleText,
    /// `state.enabled`.
    Enabled,
    /// `state.selected`.
    Selected,
    /// `state.checked`.
    Checked,
}

impl fmt::Display for Property {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Property::Role => "role",
            Property::Name => "name",
            Property::Value => "value",
            Property::VisibleText => "visible_text",
            Property::Enabled => "enabled",
            Property::Selected => "selected",
            Property::Checked => "checked",
        })
    }
}

/// A value claimed by a source.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Claim {
    /// The claiming source.
    pub source: Source,
    /// The claimed value, as text.
    pub value: String,
}

/// A disagreement between sources about one property.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Conflict {
    /// The property.
    pub property: Property,
    /// The value reported in the element.
    pub kept: Claim,
    /// The value that lost.
    pub rejected: Claim,
    /// Whether the confidence of the kept value was lowered.
    pub lowers_confidence: bool,
}
