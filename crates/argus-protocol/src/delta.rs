use serde::{Deserialize, Serialize};

use crate::{Element, ElementId, ObservationId};

/// Structural difference between two observations.
///
/// Applying the delta to observation `from` yields observation `to`
/// (up to element order).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObservationDelta {
    /// Base observation.
    pub from: ObservationId,
    /// Resulting observation.
    pub to: ObservationId,
    /// Elements present in `to` but not in `from`.
    #[serde(default)]
    pub added: Vec<Element>,
    /// Elements present in `from` but not in `to`.
    #[serde(default)]
    pub removed: Vec<ElementId>,
    /// Property changes of elements present in both.
    #[serde(default)]
    pub changed: Vec<ElementChange>,
}

/// A change of one property of one element.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ElementChange {
    /// The changed element.
    pub id: ElementId,
    /// Dot-separated path of the property in the element's JSON form,
    /// e.g. `name`, `bounds` or `state.enabled`.
    pub property: String,
    /// Previous value; `null` if the property was absent.
    pub from: serde_json::Value,
    /// New value; `null` if the property is now absent.
    pub to: serde_json::Value,
}
