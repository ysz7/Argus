use std::collections::{HashMap, HashSet};

use thiserror::Error;

use crate::{ElementId, Observation};

/// A referential invariant of an [`Observation`] is violated.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum ValidationError {
    /// Two elements share an ID.
    #[error("duplicate element id `{0}`")]
    DuplicateElementId(ElementId),

    /// An element's `parent` or `children` references an ID that is not in the
    /// observation.
    #[error("`{referenced_by}` references missing element `{missing}`")]
    MissingElement {
        /// Element whose field holds the reference.
        referenced_by: ElementId,
        /// The unknown ID.
        missing: ElementId,
    },

    /// `parent` and `children` disagree.
    #[error("`{child}` and `{parent}` disagree about their parent/child link")]
    HierarchyMismatch {
        /// The parent side of the link.
        parent: ElementId,
        /// The child side of the link.
        child: ElementId,
    },

    /// Following `parent` links from an element returns to it.
    #[error("parent chain of `{0}` contains a cycle")]
    HierarchyCycle(ElementId),

    /// A relation endpoint is not in the observation.
    #[error("relation references missing element `{0}`")]
    MissingRelationEndpoint(ElementId),

    /// An element lists no sources.
    #[error("element `{0}` has no sources")]
    NoSources(ElementId),
}

impl Observation {
    /// Checks the referential invariants of the observation.
    ///
    /// Returns every violation found, in element order.
    pub fn validate(&self) -> Result<(), Vec<ValidationError>> {
        let mut errors = Vec::new();

        let mut by_id = HashMap::with_capacity(self.elements.len());
        for element in &self.elements {
            if by_id.insert(&element.id, element).is_some() {
                errors.push(ValidationError::DuplicateElementId(element.id.clone()));
            }
        }

        let missing = |from: &ElementId, to: &ElementId| ValidationError::MissingElement {
            referenced_by: from.clone(),
            missing: to.clone(),
        };

        for element in &self.elements {
            if element.sources.is_empty() {
                errors.push(ValidationError::NoSources(element.id.clone()));
            }

            if let Some(parent_id) = &element.parent {
                match by_id.get(parent_id) {
                    None => errors.push(missing(&element.id, parent_id)),
                    Some(parent) if !parent.children.contains(&element.id) => {
                        errors.push(ValidationError::HierarchyMismatch {
                            parent: parent_id.clone(),
                            child: element.id.clone(),
                        });
                    }
                    Some(_) => {}
                }
            }

            for child_id in &element.children {
                match by_id.get(child_id) {
                    None => errors.push(missing(&element.id, child_id)),
                    Some(child) if child.parent.as_ref() != Some(&element.id) => {
                        errors.push(ValidationError::HierarchyMismatch {
                            parent: element.id.clone(),
                            child: child_id.clone(),
                        });
                    }
                    Some(_) => {}
                }
            }

            if has_parent_cycle(element.id.clone(), &by_id) {
                errors.push(ValidationError::HierarchyCycle(element.id.clone()));
            }
        }

        for relation in &self.relations {
            for endpoint in [&relation.from, &relation.to] {
                if !by_id.contains_key(endpoint) {
                    errors.push(ValidationError::MissingRelationEndpoint(endpoint.clone()));
                }
            }
        }

        if errors.is_empty() { Ok(()) } else { Err(errors) }
    }
}

fn has_parent_cycle(start: ElementId, by_id: &HashMap<&ElementId, &crate::Element>) -> bool {
    let mut seen = HashSet::new();
    let mut current = Some(start);
    while let Some(id) = current {
        if !seen.insert(id.clone()) {
            return true;
        }
        current = by_id.get(&id).and_then(|element| element.parent.clone());
    }
    false
}
