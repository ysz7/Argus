//! The tools the server offers and the instructions that come with them.

use serde_json::{Value, json};

/// Told to the client at initialization; clients show it to the model.
pub(crate) const INSTRUCTIONS: &str = "\
Argus lets you see and operate macOS applications on the user's computer.

1. `list_apps` shows the applications with windows. `observe` with an `app` \
describes its front window as text: one element per line with an id, role, \
name, `= value`, [state] and bounds (x,y wxh) in screen points. Like controls \
side by side share a line (\"row of 4 buttons: e_13 \"7\" | e_14 \"8\" …\"). \
Text fields show their selection and text styles on the following lines. A `?` \
marks an uncertain role or name.
2. Where text cannot describe the window well (no accessibility, drawn \
content, pop-ups), images of those regions come too, labelled R1, R2, … with \
their screen position.
3. Act with `click`, `type_text`, `press_keys`, `scroll` and `drag`. Address \
elements by id and pass the name you expect in `expect`: the action is refused \
if the element is something else. To click something seen only in a region \
image, pass `image` (e.g. \"R1\") and x, y in that image's pixels. Ids stay \
the same for the same element across observations.
4. Every action answers what changed (and images of regions whose pixels \
changed). Call `observe` again for a complete fresh view. Verify the result \
before saying a task is done.";

/// The target of a pointer action: an element, a point in a region image,
/// or a screen point.
fn target(prefix: &str) -> serde_json::Map<String, Value> {
    let key = |name: &str| format!("{prefix}{name}");
    let mut properties = serde_json::Map::new();
    properties.insert(
        key("element_id"),
        json!({"type": "string", "description": "id of the element, e.g. e_12"}),
    );
    properties.insert(
        key("expect"),
        json!({"type": "string", "description": "the name (or label or value) you expect the element to have; the action is refused otherwise"}),
    );
    properties.insert(
        key("image"),
        json!({"type": "string", "description": "label of a region image (R1, …): x and y are then pixels of that image"}),
    );
    properties.insert(
        key("x"),
        json!({"type": "number", "description": "x in the region image, or in screen points"}),
    );
    properties.insert(
        key("y"),
        json!({"type": "number", "description": "y in the region image, or in screen points"}),
    );
    properties
}

fn schema(properties: serde_json::Map<String, Value>, required: &[&str]) -> Value {
    json!({"type": "object", "properties": properties, "required": required, "additionalProperties": false})
}

/// The properties of a JSON object literal.
fn props(value: Value) -> serde_json::Map<String, Value> {
    match value {
        Value::Object(properties) => properties,
        _ => serde_json::Map::new(),
    }
}

fn with(
    mut properties: serde_json::Map<String, Value>,
    extra: Value,
) -> serde_json::Map<String, Value> {
    properties.extend(props(extra));
    properties
}

/// The `tools/list` answer.
pub(crate) fn list() -> Value {
    let read_only = json!({"readOnlyHint": true, "openWorldHint": false});
    let acts = json!({"readOnlyHint": false, "destructiveHint": false, "openWorldHint": false});
    json!([
        {
            "name": "list_apps",
            "title": "List applications",
            "description": "The applications that have windows on screen (names only).",
            "inputSchema": schema(serde_json::Map::new(), &[]),
            "annotations": read_only,
        },
        {
            "name": "observe",
            "title": "Observe an application",
            "description": "A complete, fresh description of an application's front window (text, plus images of regions text cannot describe). Pass `app` the first time; later calls observe the same application.",
            "inputSchema": schema(props(json!({
                "app": {"type": "string", "description": "application name (as in list_apps) or bundle id"},
            })), &[]),
            "annotations": read_only,
        },
        {
            "name": "click",
            "title": "Click",
            "description": "Click an element (by id, with `expect`), a point in a region image, or a screen point. Answers what changed.",
            "inputSchema": schema(with(target(""), json!({
                "button": {"type": "string", "enum": ["left", "right", "middle"]},
                "count": {"type": "integer", "minimum": 1, "maximum": 3, "description": "2 for a double click"},
                "modifiers": {"type": "array", "items": {"type": "string", "enum": ["cmd", "shift", "alt", "ctrl"]}},
            })), &[]),
            "annotations": acts,
        },
        {
            "name": "type_text",
            "title": "Type text",
            "description": "Type text with the keyboard. With `element_id`, clicks that element first; with `clear`, selects its content first so the text replaces it. Answers what changed.",
            "inputSchema": schema(props(json!({
                "text": {"type": "string"},
                "element_id": {"type": "string", "description": "field to click first"},
                "expect": {"type": "string", "description": "the name, label or value you expect the field to have"},
                "clear": {"type": "boolean", "description": "replace the field's content (cmd+a first)"},
            })), &["text"]),
            "annotations": acts,
        },
        {
            "name": "press_keys",
            "title": "Press keys",
            "description": "Press a key or combination, e.g. \"Return\", \"cmd+a\", \"shift+Tab\", \"Page_Down\". Answers what changed.",
            "inputSchema": schema(props(json!({
                "keys": {"type": "string"},
                "repeat": {"type": "integer", "minimum": 1, "maximum": 50},
            })), &["keys"]),
            "annotations": acts,
        },
        {
            "name": "scroll",
            "title": "Scroll",
            "description": "Scroll at an element, a point in a region image, or a screen point (default: the window's center). Answers what changed.",
            "inputSchema": schema(with(target(""), json!({
                "direction": {"type": "string", "enum": ["up", "down", "left", "right"]},
                "amount": {"type": "integer", "minimum": 1, "maximum": 30, "description": "lines, default 5"},
            })), &["direction"]),
            "annotations": acts,
        },
        {
            "name": "drag",
            "title": "Drag",
            "description": "Drag with the left button from one target to another (from_element_id / from_image + from_x, from_y / from_x, from_y; likewise to_…). Answers what changed.",
            "inputSchema": schema(with(target("from_"), Value::Object(target("to_"))), &[]),
            "annotations": acts,
        },
        {
            "name": "screenshot",
            "title": "Screenshot",
            "description": "An image of the observed application's front window. Usually unnecessary: `observe` already sends images where text is not enough.",
            "inputSchema": schema(serde_json::Map::new(), &[]),
            "annotations": read_only,
        },
    ])
}
