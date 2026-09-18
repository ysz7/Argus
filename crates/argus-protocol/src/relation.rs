use serde::{Deserialize, Serialize};

use crate::{ElementId, Score};

/// A directed relation between two elements: `from <kind> to`.
///
/// The structural hierarchy is expressed by
/// [`Element::parent`](crate::Element::parent) and
/// [`Element::children`](crate::Element::children); relations describe
/// additional, possibly non-hierarchical links.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Relation {
    /// Relation type.
    pub kind: RelationKind,
    /// Subject element.
    pub from: ElementId,
    /// Object element.
    pub to: ElementId,
    /// Confidence that the relation holds. `None` means not assessed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<Score>,
}

/// Type of a [`Relation`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum RelationKind {
    /// `from` spatially contains `to` (independent of the structural tree).
    Contains,
    /// `from` is the label of `to`.
    LabelFor,
    /// `from` belongs to row `to`.
    BelongsToRow,
    /// `from` belongs to column `to`.
    BelongsToColumn,
    /// `from` is visually aligned with `to`.
    AlignedWith,
    /// `from` is drawn on top of `to`.
    Overlays,
    /// Activating `from` opens `to` (menu, popover, dialog).
    Opens,
    /// A relation this version of the protocol does not know.
    #[serde(other)]
    Unknown,
}
