//! Conversion of raw accessibility trees into source candidates.
//!
//! The adapter applies platform semantics only: role mapping, which attribute
//! holds the label, how check states are encoded, which containers clip their
//! content. Generic cleanup (text, visibility, consistency) is done by the
//! normalization stage in `argus-core`.
//!
//! Platform-independent: operates on [`AxNode`] data only, so it is tested
//! with recorded fixtures on any OS.

use argus_protocol::{
    Bounds, CandidateId, CheckState, Confidence, ElementState, Region, Role, Score, Source,
    SourceCandidate, SourceMeta,
};

use crate::roles::{clips_children, is_text_input, map_role, subrole_name, text_in_value};
use crate::{AxFrame, AxNode, AxSnapshot, AxValue};

/// Converts a snapshot into candidates in depth-first pre-order (parents
/// before children).
///
/// Nodes without a frame cannot be grounded; they are dropped and their
/// children are attached to the nearest grounded ancestor.
pub fn candidates(snapshot: &AxSnapshot) -> Vec<SourceCandidate> {
    let mut output = Vec::new();
    let clip = snapshot.window.frame.and_then(to_bounds);
    visit(&snapshot.window, None, clip, &mut output);
    output
}

fn visit(
    node: &AxNode,
    parent: Option<CandidateId>,
    clip: Option<Bounds>,
    output: &mut Vec<SourceCandidate>,
) {
    let bounds = node.frame.and_then(to_bounds);
    let mut child_parent = parent;
    let mut child_clip = clip;

    if let Some(bounds) = bounds {
        let id = CandidateId(output.len() as u32);
        output.push(candidate(node, id, parent, bounds, clip));
        child_parent = Some(id);
        if clips_children(&node.role) {
            child_clip = Some(clip.map_or(Some(bounds), |clip| clip.intersection(&bounds)))
                .flatten()
                .or(Some(empty_at(bounds)));
        }
    }

    for child in &node.children {
        visit(child, child_parent, child_clip, output);
    }
}

fn candidate(
    node: &AxNode,
    id: CandidateId,
    parent: Option<CandidateId>,
    bounds: Bounds,
    clip: Option<Bounds>,
) -> SourceCandidate {
    let role = map_role(&node.role, node.subrole.as_deref());
    let (name, description) = name_and_description(node);
    let value = value(node, role);

    let mut state = ElementState {
        enabled: node.enabled,
        focused: node.focused,
        selected: node.selected,
        expanded: node.expanded,
        ..ElementState::default()
    };
    match role {
        Role::Checkbox | Role::RadioButton => state.checked = check_state(node.value.as_ref()),
        // Tabs report their selection through the value.
        Role::Tab if state.selected.is_none() => {
            state.selected = check_state(node.value.as_ref()).map(|c| c == CheckState::Checked);
        }
        _ => {}
    }
    if is_text_input(&node.role) {
        state.editable = Some(node.enabled.unwrap_or(true));
    } else if text_in_value(&node.role) {
        state.editable = Some(false);
    }

    // The platform is authoritative for everything it reports.
    let reported = |present: bool| present.then_some(Score::CERTAIN);
    let confidence = Confidence {
        role: reported(role != Role::Unknown),
        name: reported(name.is_some()),
        value: reported(value.is_some()),
        bounds: Some(Score::CERTAIN),
        state: reported(!state.is_unknown()),
        ..Confidence::new(Score::CERTAIN)
    };

    SourceCandidate {
        id,
        parent,
        role,
        name,
        value,
        description,
        region: Region::Screen(bounds),
        clip,
        state,
        confidence,
        relations: Vec::new(),
        meta: SourceMeta {
            source: Source::Accessibility,
            native_role: Some(match &node.subrole {
                Some(subrole) => format!("{}/{subrole}", node.role),
                None => node.role.clone(),
            }),
            native_id: node.identifier.clone().filter(|id| !id.is_empty()),
        },
    }
}

/// Chooses the name and description following macOS labelling conventions.
fn name_and_description(node: &AxNode) -> (Option<String>, Option<String>) {
    let text_value = match &node.value {
        Some(AxValue::String(text)) if text_in_value(&node.role) => non_blank(Some(text)),
        _ => None,
    };
    let title = non_blank(node.title.as_ref());
    let description = non_blank(node.description.as_ref());
    let help = non_blank(node.help.as_ref());
    let placeholder = non_blank(node.placeholder.as_ref());

    // Static text carries its text in the value; unlabelled controls (icon
    // buttons) carry their label in the description; empty text fields show
    // their placeholder.
    let name = text_value
        .or_else(|| title.clone())
        .or_else(|| description.clone())
        .or_else(|| is_text_input(&node.role).then(|| placeholder.clone()).flatten())
        .or_else(|| subrole_name(node.subrole.as_deref()).map(str::to_owned));
    let longer = description.filter(|text| Some(text) != name.as_ref()).or(help);
    (name, longer)
}

fn value(node: &AxNode, role: Role) -> Option<String> {
    if text_in_value(&node.role) || matches!(role, Role::Checkbox | Role::RadioButton | Role::Tab) {
        return None;
    }
    // Secure text fields never expose their contents.
    if node.subrole.as_deref() == Some("AXSecureTextField") {
        return None;
    }
    match node.value.as_ref()? {
        AxValue::String(text) => Some(text.clone()),
        AxValue::Number(number) if matches!(role, Role::Slider | Role::ProgressBar) => {
            Some(format_number(*number))
        }
        _ => None,
    }
}

fn check_state(value: Option<&AxValue>) -> Option<CheckState> {
    match value? {
        AxValue::Bool(true) => Some(CheckState::Checked),
        AxValue::Bool(false) => Some(CheckState::Unchecked),
        AxValue::Number(n) if *n == 0.0 => Some(CheckState::Unchecked),
        AxValue::Number(n) if *n == 1.0 => Some(CheckState::Checked),
        AxValue::Number(n) if *n == 2.0 => Some(CheckState::Mixed),
        _ => None,
    }
}

fn format_number(number: f64) -> String {
    if number.fract() == 0.0 && number.abs() < 1e15 {
        format!("{}", number as i64)
    } else {
        format!("{number}")
    }
}

/// The text, unless it is missing or blank. Cleanup happens in
/// normalization; here blankness only decides which attribute labels a node.
fn non_blank(text: Option<&String>) -> Option<String> {
    text.filter(|text| !text.trim().is_empty()).cloned()
}

fn to_bounds(frame: AxFrame) -> Option<Bounds> {
    Bounds::new(frame.x as f32, frame.y as f32, frame.width as f32, frame.height as f32).ok()
}

/// A zero-size clip: everything inside a fully clipped container is hidden.
fn empty_at(bounds: Bounds) -> Bounds {
    Bounds::new(bounds.x(), bounds.y(), 0.0, 0.0).unwrap_or(bounds)
}

#[cfg(test)]
mod tests {
    use argus_protocol::Application;

    use super::*;

    fn frame(x: f64, y: f64, width: f64, height: f64) -> Option<AxFrame> {
        Some(AxFrame { x, y, width, height })
    }

    fn node(role: &str, frame: Option<AxFrame>) -> AxNode {
        AxNode { role: role.to_owned(), frame, ..AxNode::default() }
    }

    fn snapshot(children: Vec<AxNode>) -> AxSnapshot {
        AxSnapshot {
            application: Application::default(),
            window: AxNode {
                title: Some("Settings".to_owned()),
                children,
                ..node("AXWindow", frame(0.0, 0.0, 400.0, 300.0))
            },
            truncated: false,
        }
    }

    fn only_child(children: Vec<AxNode>) -> SourceCandidate {
        let candidates = candidates(&snapshot(children));
        assert_eq!(candidates.len(), 2, "{candidates:#?}");
        candidates[1].clone()
    }

    #[test]
    fn window_is_the_root() {
        let candidates = candidates(&snapshot(Vec::new()));
        assert_eq!(candidates[0].role, Role::Window);
        assert_eq!(candidates[0].name.as_deref(), Some("Settings"));
        assert_eq!(candidates[0].parent, None);
        assert_eq!(candidates[0].clip, Bounds::new(0.0, 0.0, 400.0, 300.0).ok());
    }

    #[test]
    fn static_text_is_named_by_its_value() {
        let text = only_child(vec![AxNode {
            value: Some(AxValue::String("Delete file?".to_owned())),
            ..node("AXStaticText", frame(10.0, 10.0, 100.0, 20.0))
        }]);
        assert_eq!(text.role, Role::Text);
        assert_eq!(text.name.as_deref(), Some("Delete file?"));
        assert_eq!(text.value, None);
        assert_eq!(text.state.editable, Some(false));
    }

    #[test]
    fn icon_buttons_are_named_by_description() {
        let button = only_child(vec![AxNode {
            subrole: Some("AXCloseButton".to_owned()),
            description: Some("close button".to_owned()),
            help: Some("Close this window".to_owned()),
            enabled: Some(true),
            ..node("AXButton", frame(8.0, 4.0, 14.0, 16.0))
        }]);
        assert_eq!(button.role, Role::Button);
        assert_eq!(button.name.as_deref(), Some("close button"));
        assert_eq!(button.description.as_deref(), Some("Close this window"));
        assert_eq!(button.meta.native_role.as_deref(), Some("AXButton/AXCloseButton"));
        assert_eq!(button.confidence.role, Some(Score::CERTAIN));
    }

    #[test]
    fn window_buttons_are_named_by_subrole() {
        let zoom = only_child(vec![AxNode {
            subrole: Some("AXZoomButton".to_owned()),
            ..node("AXButton", frame(8.0, 4.0, 14.0, 16.0))
        }]);
        assert_eq!(zoom.name.as_deref(), Some("Zoom"));
    }

    #[test]
    fn checkbox_values_become_check_states() {
        for (value, expected) in [
            (AxValue::Number(0.0), CheckState::Unchecked),
            (AxValue::Number(1.0), CheckState::Checked),
            (AxValue::Number(2.0), CheckState::Mixed),
            (AxValue::Bool(true), CheckState::Checked),
        ] {
            let checkbox = only_child(vec![AxNode {
                title: Some("Remember me".to_owned()),
                value: Some(value),
                ..node("AXCheckBox", frame(10.0, 10.0, 100.0, 20.0))
            }]);
            assert_eq!(checkbox.state.checked, Some(expected));
            assert_eq!(checkbox.value, None);
        }
    }

    #[test]
    fn tabs_report_selection() {
        let tab = only_child(vec![AxNode {
            subrole: Some("AXTabButton".to_owned()),
            title: Some("General".to_owned()),
            value: Some(AxValue::Number(1.0)),
            ..node("AXRadioButton", frame(10.0, 10.0, 60.0, 20.0))
        }]);
        assert_eq!(
            (tab.role, tab.state.selected, tab.state.checked),
            (Role::Tab, Some(true), None)
        );
    }

    #[test]
    fn text_fields_expose_value_and_placeholder_but_not_secrets() {
        let empty = only_child(vec![AxNode {
            placeholder: Some("Search".to_owned()),
            value: Some(AxValue::String(String::new())),
            ..node("AXTextField", frame(10.0, 10.0, 100.0, 20.0))
        }]);
        assert_eq!(empty.name.as_deref(), Some("Search"));
        assert_eq!(empty.value.as_deref(), Some(""));
        assert_eq!(empty.state.editable, Some(true));

        let secure = only_child(vec![AxNode {
            subrole: Some("AXSecureTextField".to_owned()),
            value: Some(AxValue::String("hunter2".to_owned())),
            ..node("AXTextField", frame(10.0, 40.0, 100.0, 20.0))
        }]);
        assert_eq!(secure.value, None);
    }

    #[test]
    fn unknown_roles_have_no_role_confidence() {
        let splitter = only_child(vec![node("AXSplitter", frame(200.0, 0.0, 1.0, 300.0))]);
        assert_eq!(splitter.role, Role::Unknown);
        assert_eq!(splitter.confidence.role, None);
        assert_eq!(splitter.meta.native_role.as_deref(), Some("AXSplitter"));
    }

    #[test]
    fn frameless_nodes_are_flattened() {
        let candidates = candidates(&snapshot(vec![AxNode {
            children: vec![node("AXButton", frame(10.0, 10.0, 50.0, 20.0))],
            ..node("AXGroup", None)
        }]));
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[1].role, Role::Button);
        assert_eq!(candidates[1].parent, Some(CandidateId(0)));
    }

    #[test]
    fn scroll_areas_clip_their_content() {
        let candidates = candidates(&snapshot(vec![AxNode {
            children: vec![node("AXStaticText", frame(0.0, 150.0, 100.0, 20.0))],
            ..node("AXScrollArea", frame(0.0, 40.0, 500.0, 60.0))
        }]));
        let [window, area, text] = candidates.as_slice() else {
            panic!("{candidates:#?}");
        };
        assert_eq!(area.parent, Some(window.id));
        assert_eq!(area.clip, Bounds::new(0.0, 0.0, 400.0, 300.0).ok());
        assert_eq!(text.parent, Some(area.id));
        // The window and the scroll area both clip.
        assert_eq!(text.clip, Bounds::new(0.0, 40.0, 400.0, 60.0).ok());
    }

    #[test]
    fn content_of_hidden_scroll_areas_gets_an_empty_clip() {
        let candidates = candidates(&snapshot(vec![AxNode {
            children: vec![node("AXStaticText", frame(0.0, 600.0, 100.0, 20.0))],
            ..node("AXScrollArea", frame(0.0, 500.0, 200.0, 200.0))
        }]));
        let clip = candidates[2].clip.unwrap();
        assert_eq!((clip.width(), clip.height()), (0.0, 0.0));
    }

    #[test]
    fn keeps_native_identity() {
        let seven = only_child(vec![AxNode {
            identifier: Some("Seven".to_owned()),
            description: Some("7".to_owned()),
            ..node("AXButton", frame(10.0, 10.0, 48.0, 48.0))
        }]);
        assert_eq!(seven.meta.native_id.as_deref(), Some("Seven"));
        assert_eq!(seven.meta.source, Source::Accessibility);
    }

    #[test]
    fn slider_values_are_formatted() {
        let slider = only_child(vec![AxNode {
            value: Some(AxValue::Number(75.0)),
            ..node("AXSlider", frame(10.0, 10.0, 100.0, 20.0))
        }]);
        assert_eq!(slider.value.as_deref(), Some("75"));
    }
}
