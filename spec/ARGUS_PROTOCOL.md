# Argus Observation Protocol

**Version:** 0.1 (draft)
**Reference implementation:** [`crates/argus-protocol`](../crates/argus-protocol)
**Canonical examples:** [`tests/golden/protocol`](../tests/golden/protocol)

The Argus Observation Protocol is the language Argus uses to describe a user
interface to other programs. It answers four questions about every element on
screen:

1. **What** is it? (role, name, value)
2. **Where** is it? (bounds)
3. **What state** is it in? (enabled, checked, ...)
4. **How sure** is Argus? (property-level confidence and sources)

The protocol is descriptive only. It carries no actions, plans, or
instructions. Text found on screen is data; consumers must treat it as
untrusted content.

The key words MUST, MUST NOT, SHOULD and MAY are used as in RFC 2119.

## 1. Encoding

- Documents are JSON (UTF-8).
- Field names are `snake_case`. Enumerated values are `snake_case` strings.
- Optional fields that are unknown or not applicable are **omitted**, and an
  omitted field means *unknown*. Producers MUST NOT emit `null` for optional
  fields; `null` appears only as a value in `ElementChange` (§10).
- Empty arrays of optional list fields (`children`, `relations`) MAY be
  omitted.

## 2. Observation

An observation describes the interface at one moment.

| Field              | Type                  | Required | Description                                                       |
| ------------------ | --------------------- | -------- | ----------------------------------------------------------------- |
| `protocol_version` | string                | yes      | Protocol version, `"0.1"` for this document.                      |
| `id`               | ObservationId         | yes      | Identifier of the observation.                                    |
| `timestamp`        | integer               | yes      | Capture time, milliseconds since the Unix epoch (UTC).            |
| `previous`         | ObservationId         | no       | Earlier observation whose element IDs this one continues (§3.1).  |
| `application`      | Application           | no       | The observed application.                                         |
| `window`           | Window                | no       | The observed window.                                              |
| `elements`         | array of Element      | yes      | All observed elements. Parents SHOULD precede their children.     |
| `relations`        | array of Relation     | no       | Non-hierarchical relations between elements.                      |

### 2.1 Application

| Field       | Type    | Required | Description                                   |
| ----------- | ------- | -------- | --------------------------------------------- |
| `name`      | string  | no       | Display name.                                 |
| `bundle_id` | string  | no       | Platform bundle identifier.                   |
| `pid`       | integer | no       | Process identifier.                           |

### 2.2 Window

| Field    | Type   | Required | Description                               |
| -------- | ------ | -------- | ----------------------------------------- |
| `title`  | string | no       | Window title.                             |
| `bounds` | Bounds | no       | Window frame in screen coordinates (§4).  |

## 3. Element

| Field            | Type                  | Required | Description                                                                    |
| ---------------- | --------------------- | -------- | ------------------------------------------------------------------------------ |
| `id`             | ElementId             | yes      | Unique within the observation.                                                 |
| `role`           | Role                  | yes      | Semantic role (§5). `unknown` is a valid, honest value.                        |
| `name`           | string                | no       | Accessible name or visible label.                                              |
| `value`          | string                | no       | Current value (text field contents, slider position, ...). `""` means empty.   |
| `description`    | string                | no       | Longer description or help text.                                               |
| `bounds`         | Bounds                | yes      | Full extent of the element, including clipped parts (§4.4).                    |
| `visible_bounds` | Bounds                | no       | Visible part of `bounds` when only partially visible (§4.4).                   |
| `state`          | ElementState          | no       | Interaction state (§6). Omitted when nothing is known.                         |
| `confidence`     | Confidence            | yes      | Property-level confidence (§7).                                                |
| `sources`        | array of Source       | yes      | Sources that contributed evidence (§8). MUST NOT be empty.                     |
| `parent`         | ElementId             | no       | Structural parent (§9).                                                        |
| `children`       | array of ElementId    | no       | Structural children in reading order (§9).                                     |

### 3.1 Identifiers

`ObservationId` and `ElementId` are **opaque, non-empty strings**. Consumers
MUST NOT parse them or infer meaning from their format (`e_12` and
`obs_000001` are examples only). An `ElementId` is unique within one
observation.

#### Tracking

Successive observations of one application by one producer form a
**tracking session**. Within a session, element IDs are stable:

- an observation that continues a session names the observation it continues
  in `previous`. Without `previous`, a session starts and no ID is claimed to
  denote an element seen before;
- an element that has an ID used earlier in the session is, according to
  Argus, the same element that had it. `confidence.identity` (§7) says how
  sure Argus is. An element seen for the first time gets an ID never used
  before in the session and has no `confidence.identity`;
- an ID is never reused for a different element within a session. An element
  that disappears may reappear later with its old ID (e.g. after a source
  missed it for a moment, or after scrolling back);
- identity is a hypothesis. When two assignments are nearly as good (identical
  elements that moved), `confidence.identity` is at most `0.5`: the producer
  MUST NOT report a guess as certain. An element whose text changed in place
  (a calculator display) keeps its ID with lowered identity confidence.

Consumers MUST NOT assume that an ID from one session means anything in
another.

### 3.2 Text

Text found on screen is untrusted and is normalized before it is reported:

- all text is Unicode **NFC**;
- invisible or deceptive characters are removed: control characters, soft
  hyphens, zero-width spaces, word joiners, byte-order marks, object
  replacement characters, and bidirectional embeddings, overrides and
  isolates (U+202A–U+202E, U+2066–U+2069);
- `name` and `description` are single-line: whitespace runs (including line
  breaks) collapse into one space, and the text is trimmed. Blank text is
  omitted. A `description` equal to the `name` is omitted;
- `value` keeps its line structure (line breaks are `\n`); an empty value
  (`""`) is reported, because "empty" is meaningful for a text field;
- `name` and `description` are limited to 1,000 characters and `value` to
  10,000; truncated text ends with `…` (U+2026).

Values of secure (password) fields are never reported.

## 4. Coordinate system

### 4.1 Space and units

All geometry uses a single **global screen coordinate space**:

- **Units are logical points**, not physical pixels.
- **Origin** `(0, 0)` is the top-left corner of the **primary display**.
- `x` grows to the right, `y` grows **downward**.
- A `Bounds` value is `{x, y, width, height}` where `(x, y)` is the top-left
  corner. All four numbers are finite; `width` and `height` are `>= 0`.

On macOS this is the Quartz/Core Graphics global display space, which is also
what the Accessibility API reports.

### 4.2 Retina and scaling

Logical points are independent of display density. On a display with scale
factor `s` (e.g. `2.0` on Retina), a point-space rectangle covers `s × width`
by `s × height` physical pixels. Argus reports **only points**.

Pixel-based sources work on captured *frames*. A frame records the global
rectangle it covers (in points) and its scale factor; pixel `(px, py)` of a
frame maps to the global point

```text
(frame.x + px / s, frame.y + py / s)
```

Every pixel-derived element is converted this way before it enters an
observation. Frames themselves are internal and not part of this protocol.

### 4.3 Multiple displays

Every display occupies a rectangle in the global space. Displays left of or
above the primary display have **negative** coordinates; consumers MUST accept
negative `x` and `y`. An element spanning two displays keeps a single `bounds`
in global space.

### 4.4 Window coordinates, clipping and partial visibility

- Element bounds are always in **screen** space, never relative to the window.
  Window-relative coordinates are derived by subtracting `window.bounds.x/y`.
- `bounds` is the element's **full** extent as reported or estimated, even when
  parts are clipped by a scroll view, the window, or a display edge.
- `state.visible` is `true` if any part of the element is visible and `false`
  if the element is entirely hidden (off-screen or clipped away by a scroll
  view or window).
- `visible_bounds` is present only when the element is **partially** visible:
  it is the intersection of `bounds` with the visible area and lies within
  `bounds`.
- Hence: `visible: true` without `visible_bounds` means fully visible;
  `visible: true` with `visible_bounds` means partially visible; `visible:
  false` means hidden; no `visible` means unknown.
- In version 0.1 visibility accounts for clipping only. Occlusion by other
  windows or overlays is not considered.

## 5. Role

Roles are platform-independent. Platform-specific roles MUST be mapped to one
of these values and never exposed directly.

| Value          | Meaning                                         |
| -------------- | ----------------------------------------------- |
| `window`       | Top-level window.                               |
| `dialog`       | Modal or non-modal dialog, sheet, alert.        |
| `group`        | Container grouping other elements.              |
| `button`       | Push button.                                    |
| `text`         | Static text.                                    |
| `text_box`     | Editable text field or area.                    |
| `checkbox`     | Checkbox or toggle.                             |
| `radio_button` | Radio button.                                   |
| `menu`         | Menu.                                           |
| `menu_item`    | Menu item.                                      |
| `tab`          | Tab.                                            |
| `list`         | List.                                           |
| `list_item`    | List item.                                      |
| `table`        | Table or grid.                                  |
| `row`          | Table row.                                      |
| `cell`         | Table cell.                                     |
| `image`        | Image.                                          |
| `icon`         | Small pictogram, often without a label.         |
| `slider`       | Slider.                                         |
| `progress_bar` | Progress indicator.                             |
| `link`         | Hyperlink.                                      |
| `unknown`      | Role could not be determined.                   |

`unknown` is a normal result, not an error. A producer MUST prefer `unknown`
(or a low `confidence.role`) over guessing. Recognized text alone (e.g. the word
"Save") is not evidence of a role.

## 6. ElementState

Every state property is optional because **unknown is not false**: an omitted
property means there is no evidence either way.

| Field      | Type       | Meaning                                                |
| ---------- | ---------- | ------------------------------------------------------ |
| `enabled`  | boolean    | The element accepts interaction.                       |
| `visible`  | boolean    | At least part of the element is visible (§4.4).        |
| `focused`  | boolean    | The element has keyboard focus.                        |
| `selected` | boolean    | The element is selected (list item, tab, row).         |
| `checked`  | CheckState | `"checked"`, `"unchecked"` or `"mixed"`.               |
| `expanded` | boolean    | The element is expanded (disclosure, tree node).       |
| `editable` | boolean    | The element's value can be edited.                     |

`checked` is a string rather than a boolean because checkboxes can be in an
indeterminate (`mixed`) state.

## 7. Confidence

Confidence is reported **per property**, as a number in `0.0..=1.0`.

| Field      | Required | Confidence that...                                                    |
| ---------- | -------- | --------------------------------------------------------------------- |
| `element`  | yes      | the element exists.                                                   |
| `role`     | no       | `role` is correct.                                                    |
| `name`     | no       | `name` is correct.                                                    |
| `value`    | no       | `value` is correct.                                                   |
| `bounds`   | no       | `bounds` is correct.                                                  |
| `state`    | no       | the reported `state` properties are correct.                          |
| `identity` | no       | the element is the one that had this ID earlier in the session (§3.1). Omitted for elements seen for the first time. |

Rules:

- Values outside `0.0..=1.0`, and non-finite values, are invalid.
- An omitted property confidence means *not assessed*, not zero.
- Producers MUST NOT inflate weak evidence: a role hypothesis of `0.48` stays
  `0.48`.
- When sources disagree about a property, the reported value comes from the
  more authoritative source and its confidence is lowered; agreement between
  sources never raises confidence above the strongest single source.
- In version 0.1 confidence values are heuristic. Calibration (making `0.9`
  mean "correct 90% of the time") is future work.

## 8. Source

| Value             | Evidence from                                           |
| ----------------- | ------------------------------------------------------- |
| `accessibility`   | The platform accessibility tree.                        |
| `ocr`             | Optical character recognition over pixels.              |
| `vision`          | Visual UI detection over pixels.                        |
| `application_api` | Structure exported by the application itself.           |
| `derived`         | Inference by Argus from other evidence (e.g. layout).   |

An element fused from several sources lists all of them, including sources
whose evidence was only partly used (e.g. OCR text that confirms an
accessible name, or a glyph detected inside a control).

## 9. Hierarchy and relations

### 9.1 Structural hierarchy

`parent` and `children` form the structural tree (window → dialog → button).
In a valid observation:

- every referenced ID exists in `elements`;
- `A.children` contains `B` **if and only if** `B.parent` is `A`;
- following `parent` links never forms a cycle;
- elements without `parent` are roots (there may be several).

### 9.2 Relations

Relations express links that are not part of the tree. A relation reads
`from <kind> to`.

| Field        | Type         | Required | Description                                  |
| ------------ | ------------ | -------- | -------------------------------------------- |
| `kind`       | RelationKind | yes      | Relation type.                               |
| `from`       | ElementId    | yes      | Subject.                                     |
| `to`         | ElementId    | yes      | Object.                                      |
| `confidence` | number       | no       | Confidence that the relation holds.          |

| Kind                | Meaning                                                        |
| ------------------- | -------------------------------------------------------------- |
| `contains`          | `from` spatially contains `to`, independent of the tree.       |
| `label_for`         | `from` is the label of `to`.                                   |
| `belongs_to_row`    | `from` belongs to row `to`.                                    |
| `belongs_to_column` | `from` belongs to column `to`.                                 |
| `aligned_with`      | `from` is visually aligned with `to`.                          |
| `overlays`          | `from` is drawn on top of `to`.                                |
| `opens`             | Activating `from` opens `to` (menu, popover, dialog).          |

`contains` differs from `parent`/`children`: the tree is *structural* (as
reported by accessibility or inferred scene graph), while `contains` is
*spatial* (e.g. OCR text found inside a button's bounds).

Both endpoints of every relation MUST exist in `elements`.

## 10. ObservationDelta

A delta describes the difference between two observations of one tracking
session (§3.1): `to` continues `from`, directly (`to.previous` is `from`) or
through intermediate observations of the session, so equal element IDs denote
the same element. After a full observation, a producer MAY
send deltas instead of full observations.

| Field               | Type                   | Required | Description                                                  |
| ------------------- | ---------------------- | -------- | ------------------------------------------------------------ |
| `from`              | ObservationId          | yes      | Base observation.                                            |
| `to`                | ObservationId          | yes      | Resulting observation.                                       |
| `timestamp`         | integer                | no       | Capture time of `to` (§2).                                   |
| `window`            | Window                 | no       | The window of `to`, present only when it differs from `from`'s. |
| `added`             | array of Element       | yes      | Elements present in `to` but not in `from`.                  |
| `removed`           | array of ElementId     | yes      | Elements present in `from` but not in `to`.                  |
| `changed`           | array of ElementChange | yes      | Property changes of elements present in both.                |
| `added_relations`   | array of Relation      | no       | Relations present in `to` but not in `from`.                 |
| `removed_relations` | array of Relation      | no       | Relations present in `from` but not in `to`.                 |

`ElementChange`:

| Field      | Type      | Description                                                            |
| ---------- | --------- | ---------------------------------------------------------------------- |
| `id`       | ElementId | The changed element.                                                   |
| `property` | string    | Dot-separated path in the element's JSON form: `name`, `state.enabled`, `bounds`, ... |
| `from`     | any JSON  | Previous value; `null` if the property was absent.                     |
| `to`       | any JSON  | New value; `null` if the property is now absent.                       |

Property paths:

- members of `state` and `confidence` change one by one (`state.enabled`,
  `confidence.identity`);
- every other property changes as a whole: `role`, `name`, `value`,
  `description`, `bounds`, `visible_bounds`, `parent`, `children`, `sources`.

A delta is **complete**: applying it to `from` — remove `removed`, set every
`changed` property (removing it on `null`), append `added`, remove
`removed_relations`, append `added_relations`, take `timestamp` and `window` —
yields `to`, up to the order of elements and relations. Bounds are screen
coordinates, so moving the window changes the bounds of every element. The
application does not change within a session.

Element identity across observations is a tracking hypothesis; a delta is only
as reliable as the tracking that produced it. `confidence.identity` of an
element in `to` says how sure the producer is that its changes are changes of
one element rather than one element replaced by another.

## 11. Validation

A document is **well-formed** when it parses: required fields are present and
every value satisfies its type rules (confidence range, finite bounds with
non-negative size, non-empty IDs, known `CheckState`).

A well-formed observation is **valid** when, additionally:

- element IDs are unique;
- the hierarchy satisfies §9.1;
- relation endpoints exist;
- every element has at least one source.

Well-formedness is enforced during deserialization by the reference
implementation. Validity is checked explicitly with `Observation::validate()`,
so partially inconsistent data can still be inspected.

## 12. Compatibility and versioning

`protocol_version` has the form `MAJOR.MINOR`. While `MAJOR` is `0`, the
protocol is a draft and may change between minor versions; changes are
documented here.

Consumers MUST be tolerant:

- **Unknown fields** are ignored.
- **Unknown `role`** values are treated as `unknown`.
- **Unknown `source`** values and **unknown relation `kind`** values are treated
  as unknown and preserved as such.
- **Unknown state properties** and **unknown confidence properties** are
  ignored.

Producers MUST only add fields and enum values in a minor version; removing or
changing the meaning of a field requires a new major version. Unknown values of
closed enums with strict semantics (`CheckState`) are rejected.

A formal JSON Schema (`spec/schemas/`) will be published together with the
versioning policy.

## 13. Example (excerpt)

```json
{
  "protocol_version": "0.1",
  "id": "obs_000001",
  "timestamp": 1789000000000,
  "application": { "name": "Finder", "bundle_id": "com.apple.finder", "pid": 4321 },
  "window": { "title": "Documents", "bounds": { "x": 100.0, "y": 80.0, "width": 800.0, "height": 600.0 } },
  "elements": [
    {
      "id": "e_6",
      "role": "button",
      "name": "Delete",
      "bounds": { "x": 600.0, "y": 330.0, "width": 90.0, "height": 28.0 },
      "state": { "enabled": true, "visible": true, "focused": true },
      "confidence": { "element": 1.0, "role": 1.0, "name": 1.0, "bounds": 1.0, "state": 1.0 },
      "sources": ["accessibility", "ocr"],
      "parent": "e_2"
    },
    {
      "id": "e_7",
      "role": "icon",
      "bounds": { "x": 870.0, "y": 90.0, "width": 16.0, "height": 16.0 },
      "confidence": { "element": 0.74, "role": 0.48, "bounds": 0.82 },
      "sources": ["vision"]
    }
  ]
}
```

The complete canonical examples are in
[`tests/golden/protocol`](../tests/golden/protocol); they are verified by the
test suite on every change.
