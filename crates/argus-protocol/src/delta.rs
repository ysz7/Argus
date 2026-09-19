use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use thiserror::Error;

use crate::{Element, ElementId, Observation, ObservationId, Relation, Timestamp, Window};

/// Structural difference between two observations of one tracking session.
///
/// Applying the delta to observation `from` ([`ObservationDelta::apply`])
/// yields observation `to`, up to the order of elements and relations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObservationDelta {
    /// Base observation.
    pub from: ObservationId,
    /// Resulting observation.
    pub to: ObservationId,
    /// Capture time of `to`. `None` means unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<Timestamp>,
    /// The window of `to`, present only when it differs from the window of
    /// `from` (e.g. it moved or its title changed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<Window>,
    /// Elements present in `to` but not in `from`.
    #[serde(default)]
    pub added: Vec<Element>,
    /// Elements present in `from` but not in `to`.
    #[serde(default)]
    pub removed: Vec<ElementId>,
    /// Property changes of elements present in both.
    #[serde(default)]
    pub changed: Vec<ElementChange>,
    /// Relations present in `to` but not in `from`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub added_relations: Vec<Relation>,
    /// Relations present in `from` but not in `to`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed_relations: Vec<Relation>,
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

/// A delta cannot be applied to an observation.
#[derive(Debug, Clone, PartialEq, Error)]
#[non_exhaustive]
pub enum DeltaError {
    /// The delta starts from another observation.
    #[error("the delta starts from `{expected}`, not from `{actual}`")]
    WrongBase {
        /// The delta's `from`.
        expected: ObservationId,
        /// The observation it was applied to.
        actual: ObservationId,
    },
    /// A removed or changed element is not in the base observation.
    #[error("element `{0}` is not in the base observation")]
    UnknownElement(ElementId),
    /// An added element is already in the base observation.
    #[error("element `{0}` is already in the base observation")]
    DuplicateElement(ElementId),
    /// A removed relation is not in the base observation.
    #[error("a removed relation is not in the base observation")]
    UnknownRelation,
    /// A change sets a property to a value it cannot have.
    #[error("invalid change of `{property}` of `{id}`: {reason}")]
    InvalidChange {
        /// The element.
        id: ElementId,
        /// The property path.
        property: String,
        /// Why the value is invalid.
        reason: String,
    },
}

/// Objects whose members are compared (and changed) one by one; every other
/// property is compared as a whole.
const NESTED: [&str; 2] = ["state", "confidence"];

/// The order in which the changes of one element are listed.
const PROPERTY_ORDER: [&str; 24] = [
    "role",
    "name",
    "value",
    "text",
    "description",
    "bounds",
    "visible_bounds",
    "state.enabled",
    "state.visible",
    "state.focused",
    "state.selected",
    "state.checked",
    "state.expanded",
    "state.editable",
    "parent",
    "children",
    "sources",
    "confidence.element",
    "confidence.role",
    "confidence.name",
    "confidence.value",
    "confidence.bounds",
    "confidence.state",
    "confidence.identity",
];

impl ObservationDelta {
    /// The difference between `from` and `to`, element by element ID.
    ///
    /// Element IDs are compared as they are, so the delta is meaningful only
    /// when `to` continues the tracking session of `from` (see
    /// [`Observation::previous`]). The application is assumed to be the
    /// same, as it is within a session.
    pub fn between(from: &Observation, to: &Observation) -> Self {
        let before: HashMap<&ElementId, &Element> =
            from.elements.iter().map(|element| (&element.id, element)).collect();
        let after: HashSet<&ElementId> = to.elements.iter().map(|element| &element.id).collect();

        let mut added = Vec::new();
        let mut changed = Vec::new();
        for element in &to.elements {
            match before.get(&element.id) {
                Some(old) => changed.extend(element_changes(old, element)),
                None => added.push(element.clone()),
            }
        }
        let removed = from
            .elements
            .iter()
            .filter(|element| !after.contains(&element.id))
            .map(|element| element.id.clone())
            .collect();

        let key = |relation: &Relation| serde_json::to_string(relation).expect("serializable");
        let old_relations: HashSet<String> = from.relations.iter().map(key).collect();
        let new_relations: HashSet<String> = to.relations.iter().map(key).collect();

        Self {
            from: from.id.clone(),
            to: to.id.clone(),
            timestamp: Some(to.timestamp),
            window: (from.window != to.window).then(|| to.window.clone().unwrap_or_default()),
            added,
            removed,
            changed,
            added_relations: to
                .relations
                .iter()
                .filter(|relation| !old_relations.contains(&key(relation)))
                .cloned()
                .collect(),
            removed_relations: from
                .relations
                .iter()
                .filter(|relation| !new_relations.contains(&key(relation)))
                .cloned()
                .collect(),
        }
    }

    /// Whether nothing but the observation and its time changed.
    pub fn is_empty(&self) -> bool {
        self.window.is_none()
            && self.added.is_empty()
            && self.removed.is_empty()
            && self.changed.is_empty()
            && self.added_relations.is_empty()
            && self.removed_relations.is_empty()
    }

    /// Applies the delta to `from`, producing observation `to`. Added
    /// elements and relations are appended; the result continues `from`
    /// ([`Observation::previous`]).
    pub fn apply(&self, from: &Observation) -> Result<Observation, DeltaError> {
        if from.id != self.from {
            return Err(DeltaError::WrongBase {
                expected: self.from.clone(),
                actual: from.id.clone(),
            });
        }
        let mut to = from.clone();
        to.id = self.to.clone();
        to.previous = Some(from.id.clone());
        if let Some(timestamp) = self.timestamp {
            to.timestamp = timestamp;
        }
        if let Some(window) = &self.window {
            to.window = Some(window.clone());
        }

        let removed: HashSet<&ElementId> = self.removed.iter().collect();
        for id in &removed {
            if !from.elements.iter().any(|element| &element.id == *id) {
                return Err(DeltaError::UnknownElement((*id).clone()));
            }
        }
        to.elements.retain(|element| !removed.contains(&element.id));

        let index: HashMap<ElementId, usize> = to
            .elements
            .iter()
            .enumerate()
            .map(|(position, element)| (element.id.clone(), position))
            .collect();
        let mut edited: BTreeMap<usize, Value> = BTreeMap::new();
        for change in &self.changed {
            let &position = index
                .get(&change.id)
                .ok_or_else(|| DeltaError::UnknownElement(change.id.clone()))?;
            let json = edited.entry(position).or_insert_with(|| {
                serde_json::to_value(&to.elements[position]).expect("serializable")
            });
            set_property(json, &change.property, change.to.clone());
        }
        for (position, json) in edited {
            let id = to.elements[position].id.clone();
            to.elements[position] =
                serde_json::from_value(json).map_err(|error| DeltaError::InvalidChange {
                    property: self
                        .changed
                        .iter()
                        .filter(|change| change.id == id)
                        .map(|change| change.property.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                    id,
                    reason: error.to_string(),
                })?;
        }

        for element in &self.added {
            if index.contains_key(&element.id) || removed.contains(&element.id) {
                return Err(DeltaError::DuplicateElement(element.id.clone()));
            }
            to.elements.push(element.clone());
        }

        for relation in &self.removed_relations {
            let position = to
                .relations
                .iter()
                .position(|existing| existing == relation)
                .ok_or(DeltaError::UnknownRelation)?;
            to.relations.remove(position);
        }
        to.relations.extend(self.added_relations.iter().cloned());
        Ok(to)
    }
}

/// The changes of the properties of one element, in [`PROPERTY_ORDER`]
/// (unknown properties last, by name).
fn element_changes(old: &Element, new: &Element) -> Vec<ElementChange> {
    let (before, after) = (properties(old), properties(new));
    let mut paths: Vec<&String> = before.keys().chain(after.keys()).collect();
    paths.sort_by_key(|path| {
        (PROPERTY_ORDER.iter().position(|known| known == path).unwrap_or(usize::MAX), *path)
    });
    paths.dedup();
    paths
        .into_iter()
        .filter_map(|path| {
            let from = before.get(path).cloned().unwrap_or(Value::Null);
            let to = after.get(path).cloned().unwrap_or(Value::Null);
            (from != to).then(|| ElementChange {
                id: new.id.clone(),
                property: path.clone(),
                from,
                to,
            })
        })
        .collect()
}

/// The properties of an element by path, as in its JSON form (absent
/// properties are missing).
fn properties(element: &Element) -> BTreeMap<String, Value> {
    let Value::Object(object) = serde_json::to_value(element).expect("serializable") else {
        unreachable!("elements serialize to objects");
    };
    let mut properties = BTreeMap::new();
    for (key, value) in object {
        match value {
            _ if key == "id" => {}
            Value::Object(members) if NESTED.contains(&key.as_str()) => {
                for (member, value) in members {
                    properties.insert(format!("{key}.{member}"), value);
                }
            }
            value => {
                properties.insert(key, value);
            }
        }
    }
    properties
}

/// Sets the property at `path` of an element's JSON form; `null` removes it.
fn set_property(element: &mut Value, path: &str, value: Value) {
    let Value::Object(object) = element else { return };
    let (object, key) = match path.split_once('.') {
        Some((outer, inner)) => {
            let nested = object.entry(outer).or_insert_with(|| Value::Object(Map::new()));
            let Value::Object(nested) = nested else { return };
            (nested, inner)
        }
        None => (object, path),
    };
    if value.is_null() {
        object.remove(key);
    } else {
        object.insert(key.to_owned(), value);
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::{Bounds, CheckState, Confidence, ElementState, RelationKind, Role, Score, Source};

    fn element(id: &str, role: Role, name: Option<&str>) -> Element {
        Element {
            id: ElementId::new(id).unwrap(),
            role,
            name: name.map(str::to_owned),
            value: None,
            text: None,
            description: None,
            bounds: Bounds::new(0.0, 0.0, 10.0, 10.0).unwrap(),
            visible_bounds: None,
            state: ElementState { enabled: Some(true), ..ElementState::default() },
            confidence: Confidence::new(Score::CERTAIN),
            sources: vec![Source::Accessibility],
            parent: None,
            children: Vec::new(),
        }
    }

    fn observation(id: &str, elements: Vec<Element>) -> Observation {
        let mut observation = Observation::new(ObservationId::new(id).unwrap(), Timestamp(1));
        observation.elements = elements;
        observation
    }

    #[test]
    fn lists_added_removed_and_changed_elements() {
        let from = observation(
            "obs_1",
            vec![element("e_1", Role::Button, Some("Save")), element("e_2", Role::Text, None)],
        );
        let mut save = element("e_1", Role::Button, Some("Saved"));
        save.state.enabled = Some(false);
        save.state.checked = Some(CheckState::Checked);
        let mut to = observation("obs_2", vec![save, element("e_3", Role::Dialog, None)]);
        to.timestamp = Timestamp(2);

        let delta = ObservationDelta::between(&from, &to);
        assert_eq!(delta.added.len(), 1);
        assert_eq!(delta.added[0].id.as_str(), "e_3");
        assert_eq!(delta.removed, [ElementId::new("e_2").unwrap()]);
        let changes: Vec<_> = delta
            .changed
            .iter()
            .map(|change| (change.property.as_str(), change.from.clone(), change.to.clone()))
            .collect();
        assert_eq!(
            changes,
            [
                ("name", json!("Save"), json!("Saved")),
                ("state.enabled", json!(true), json!(false)),
                ("state.checked", json!(null), json!("checked")),
            ]
        );
        assert_eq!(delta.timestamp, Some(Timestamp(2)));
        assert!(!delta.is_empty());

        let applied = delta.apply(&from).unwrap();
        assert_eq!(applied.elements, to.elements);
        assert_eq!(applied.previous.as_ref(), Some(&from.id));
    }

    #[test]
    fn an_unchanged_interface_has_an_empty_delta() {
        let from = observation("obs_1", vec![element("e_1", Role::Button, Some("Save"))]);
        let to = observation("obs_2", from.elements.clone());
        assert!(ObservationDelta::between(&from, &to).is_empty());
    }

    #[test]
    fn relations_and_window_changes_are_included() {
        let mut from = observation(
            "obs_1",
            vec![element("e_1", Role::Text, Some("Name")), element("e_2", Role::TextBox, None)],
        );
        from.window = Some(Window { title: Some("A".to_owned()), bounds: None });
        let mut to = from.clone();
        to.id = ObservationId::new("obs_2").unwrap();
        to.window = Some(Window { title: Some("B".to_owned()), bounds: None });
        to.relations.push(Relation {
            kind: RelationKind::LabelFor,
            from: ElementId::new("e_1").unwrap(),
            to: ElementId::new("e_2").unwrap(),
            confidence: None,
        });

        let delta = ObservationDelta::between(&from, &to);
        assert_eq!(delta.window.as_ref().and_then(|w| w.title.as_deref()), Some("B"));
        assert_eq!(delta.added_relations.len(), 1);
        let applied = delta.apply(&from).unwrap();
        assert_eq!((applied.window, applied.relations), (to.window.clone(), to.relations.clone()));

        let back = ObservationDelta::between(&to, &from);
        assert_eq!(back.removed_relations.len(), 1);
        assert!(back.apply(&to).unwrap().relations.is_empty());
    }

    #[test]
    fn inconsistent_deltas_are_rejected() {
        let from = observation("obs_1", vec![element("e_1", Role::Button, Some("Save"))]);
        let mut delta = ObservationDelta::between(&from, &from);
        let other = observation("obs_9", Vec::new());
        assert!(matches!(delta.apply(&other), Err(DeltaError::WrongBase { .. })));

        delta.removed = vec![ElementId::new("e_404").unwrap()];
        assert_eq!(
            delta.apply(&from),
            Err(DeltaError::UnknownElement(ElementId::new("e_404").unwrap()))
        );

        delta.removed.clear();
        delta.changed = vec![ElementChange {
            id: ElementId::new("e_1").unwrap(),
            property: "confidence.element".to_owned(),
            from: json!(1.0),
            to: json!(7.0),
        }];
        assert!(matches!(delta.apply(&from), Err(DeltaError::InvalidChange { .. })));

        delta.changed.clear();
        delta.added = from.elements.clone();
        assert!(matches!(delta.apply(&from), Err(DeltaError::DuplicateElement(_))));
    }
}
