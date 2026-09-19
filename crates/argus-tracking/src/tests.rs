//! Tracking scenarios.

use argus_protocol::{
    Application, Bounds, Confidence, Element, ElementId, ElementState, Observation, ObservationId,
    Relation, RelationKind, Role, Score, Source, Timestamp, Window,
};

use crate::{MEMORY, Tracker, UNCERTAIN};

/// One element of a test interface: role, name, bounds relative to the
/// window, parent (index into the list) and source.
#[derive(Clone)]
struct Spec {
    role: Role,
    name: Option<&'static str>,
    bounds: (f32, f32, f32, f32),
    parent: Option<usize>,
    source: Source,
}

fn ax(role: Role, name: &'static str, bounds: (f32, f32, f32, f32), parent: usize) -> Spec {
    Spec { role, name: Some(name), bounds, parent: Some(parent), source: Source::Accessibility }
}

fn window() -> Spec {
    Spec {
        role: Role::Window,
        name: Some("Demo"),
        bounds: (0.0, 0.0, 600.0, 400.0),
        parent: None,
        source: Source::Accessibility,
    }
}

/// An observation of `specs` in a window at `origin`, with fresh
/// (untracked) IDs as assembly produces them.
fn observe(number: u32, origin: (f32, f32), specs: &[Spec]) -> Observation {
    observe_app(number, 1, origin, specs)
}

fn observe_app(number: u32, pid: u32, origin: (f32, f32), specs: &[Spec]) -> Observation {
    let id = |index: usize| ElementId::new(format!("e_{}", index + 1)).unwrap();
    let mut elements: Vec<Element> = specs
        .iter()
        .enumerate()
        .map(|(index, spec)| {
            let (x, y, width, height) = spec.bounds;
            Element {
                id: id(index),
                role: spec.role,
                name: spec.name.map(str::to_owned),
                value: None,
                description: None,
                bounds: Bounds::new(origin.0 + x, origin.1 + y, width, height).unwrap(),
                visible_bounds: None,
                state: ElementState::default(),
                confidence: Confidence::new(Score::CERTAIN),
                sources: vec![spec.source],
                parent: spec.parent.map(id),
                children: Vec::new(),
            }
        })
        .collect();
    for (index, spec) in specs.iter().enumerate() {
        if let Some(parent) = spec.parent {
            elements[parent].children.push(id(index));
        }
    }
    let mut observation =
        Observation::new(ObservationId::new(format!("obs_{number}")).unwrap(), Timestamp(0));
    observation.application = Some(Application { pid: Some(pid), ..Application::default() });
    observation.window = Some(Window {
        title: Some("Demo".to_owned()),
        bounds: Some(Bounds::new(origin.0, origin.1, 600.0, 400.0).unwrap()),
    });
    observation.elements = elements;
    observation
}

fn track(tracker: &mut Tracker, mut observation: Observation) -> Observation {
    tracker.track(&mut observation, &[]);
    observation.validate().unwrap();
    observation
}

/// The ID of the element named `name`.
fn id_of<'a>(observation: &'a Observation, name: &str) -> &'a str {
    element(observation, name).id.as_str()
}

fn element<'a>(observation: &'a Observation, name: &str) -> &'a Element {
    observation
        .elements
        .iter()
        .find(|e| e.name.as_deref() == Some(name))
        .unwrap_or_else(|| panic!("no element named {name}"))
}

fn identity(element: &Element) -> f32 {
    element.confidence.identity.expect("identity is assessed").get()
}

fn toolbar() -> Vec<Spec> {
    vec![
        window(),
        ax(Role::Group, "Toolbar", (0.0, 0.0, 600.0, 40.0), 0),
        ax(Role::Button, "Save", (10.0, 8.0, 60.0, 24.0), 1),
        ax(Role::TextBox, "Search", (400.0, 8.0, 180.0, 24.0), 1),
    ]
}

#[test]
fn the_first_observation_starts_a_session() {
    let mut tracker = Tracker::new();
    let mut observation = observe(1, (100.0, 100.0), &toolbar());
    let report = tracker.track(&mut observation, &[]);
    assert!(!report.continued);
    assert_eq!(report.new, 4);
    assert_eq!(observation.previous, None);
    let ids: Vec<_> = observation.elements.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids, ["e_1", "e_2", "e_3", "e_4"], "the same IDs assembly gives");
    assert!(observation.elements.iter().all(|e| e.confidence.identity.is_none()));
}

#[test]
fn an_unchanged_interface_keeps_every_id() {
    let mut tracker = Tracker::new();
    let first = track(&mut tracker, observe(1, (100.0, 100.0), &toolbar()));
    let mut second = observe(2, (100.0, 100.0), &toolbar());
    let report = tracker.track(&mut second, &[]);

    assert!(report.continued);
    assert_eq!((report.kept, report.new, report.lost, report.uncertain), (4, 0, 0, 0));
    assert_eq!(second.previous.as_ref().map(ObservationId::as_str), Some("obs_1"));
    for (a, b) in first.elements.iter().zip(&second.elements) {
        assert_eq!(a.id, b.id);
        assert_eq!(identity(b), 1.0);
    }
}

/// Moving the window moves nothing inside it.
#[test]
fn a_moved_window_keeps_every_id() {
    let mut tracker = Tracker::new();
    let first = track(&mut tracker, observe(1, (100.0, 100.0), &toolbar()));
    let second = track(&mut tracker, observe(2, (340.0, 212.0), &toolbar()));
    for (a, b) in first.elements.iter().zip(&second.elements) {
        assert_eq!(a.id, b.id);
        assert_eq!(identity(b), 1.0);
    }
}

/// The roadmap example: a dialog appears; existing controls keep their IDs,
/// the dialog gets new ones.
#[test]
fn a_new_dialog_gets_new_ids() {
    let mut tracker = Tracker::new();
    let first = track(&mut tracker, observe(1, (0.0, 0.0), &toolbar()));
    let mut specs = toolbar();
    specs.push(ax(Role::Dialog, "Saved", (150.0, 100.0, 300.0, 150.0), 0));
    specs.push(ax(Role::Button, "OK", (360.0, 210.0, 70.0, 24.0), 4));
    let second = track(&mut tracker, observe(2, (0.0, 0.0), &specs));

    for name in ["Demo", "Toolbar", "Save", "Search"] {
        assert_eq!(id_of(&first, name), id_of(&second, name), "{name}");
    }
    assert_eq!(id_of(&second, "Saved"), "e_5");
    assert_eq!(id_of(&second, "OK"), "e_6");
    assert!(element(&second, "OK").confidence.identity.is_none());
    let dialog = element(&second, "Saved");
    assert_eq!(element(&second, "OK").parent.as_ref(), Some(&dialog.id), "references renamed");
}

#[test]
fn ids_are_never_reused_and_are_restored_within_memory() {
    let mut tracker = Tracker::new();
    let full = toolbar();
    let without_save: Vec<Spec> = vec![full[0].clone(), full[1].clone(), full[3].clone()];
    let first = track(&mut tracker, observe(1, (0.0, 0.0), &full));
    let save = id_of(&first, "Save").to_owned();

    // Save is missed for a moment (e.g. OCR skipped it) and comes back.
    let mut gone = observe(2, (0.0, 0.0), &without_save);
    let report = tracker.track(&mut gone, &[]);
    assert_eq!((report.kept, report.lost), (3, 1));
    let mut back = observe(3, (0.0, 0.0), &full);
    let report = tracker.track(&mut back, &[]);
    assert_eq!((report.kept, report.restored, report.new), (3, 1, 0));
    assert_eq!(id_of(&back, "Save"), save);

    // Gone for longer than the memory: a new element with a new ID.
    for number in 0..=MEMORY {
        track(&mut tracker, observe(4 + number, (0.0, 0.0), &without_save));
    }
    let late = track(&mut tracker, observe(20, (0.0, 0.0), &full));
    assert_eq!(id_of(&late, "Save"), "e_5");
}

/// A label that changes in place is the same element, with less certainty.
#[test]
fn text_changed_in_place_keeps_its_id() {
    let mut tracker = Tracker::new();
    let display = |text: &'static str| {
        vec![window(), ax(Role::Text, text, (20.0, 20.0, 200.0, 40.0), 0), {
            ax(Role::Button, "7", (20.0, 80.0, 50.0, 50.0), 0)
        }]
    };
    let first = track(&mut tracker, observe(1, (0.0, 0.0), &display("0")));
    let second = track(&mut tracker, observe(2, (0.0, 0.0), &display("7")));
    let (before, after) = (&first.elements[1], &second.elements[1]);
    assert_eq!(before.id, after.id);
    let confidence = identity(after);
    assert!((0.5..UNCERTAIN).contains(&confidence), "{confidence}");
    assert_eq!(identity(&second.elements[2]), 1.0, "the key did not change");
}

/// Calculator's display: right-aligned text in its own container, whose
/// width follows the text.
#[test]
fn a_display_keeps_its_id_while_its_text_changes() {
    let display = |text: &'static str, width: f32| {
        vec![
            window(),
            Spec {
                role: Role::Group,
                name: None,
                bounds: (10.0, 86.0, 210.0, 42.0),
                parent: Some(0),
                source: Source::Accessibility,
            },
            ax(Role::Text, text, (220.0 - width, 89.0, width, 36.0), 1),
            ax(Role::Button, "Clear", (64.0, 133.0, 48.0, 48.0), 0),
            ax(Role::Button, "Percent", (118.0, 133.0, 48.0, 48.0), 0),
        ]
    };
    let mut tracker = Tracker::new();
    let mut previous = track(&mut tracker, observe(1, (0.0, 0.0), &display("7 + 57", 72.0)));
    for (number, (text, width)) in
        [("7 +", 36.0), ("0", 19.0), ("7 × 6", 56.0)].into_iter().enumerate()
    {
        let next =
            track(&mut tracker, observe(number as u32 + 2, (0.0, 0.0), &display(text, width)));
        assert_eq!(previous.elements[1].id, next.elements[1].id, "{text}: container");
        assert_eq!(previous.elements[2].id, next.elements[2].id, "{text}: display");
        assert!(identity(&next.elements[2]) < UNCERTAIN, "{text}: the text changed");
        previous = next;
    }
}

#[test]
fn another_application_starts_a_new_session() {
    let mut tracker = Tracker::new();
    track(&mut tracker, observe_app(1, 1, (0.0, 0.0), &toolbar()));
    let mut other = observe_app(2, 2, (0.0, 0.0), &toolbar());
    let report = tracker.track(&mut other, &[]);
    assert!(!report.continued);
    assert_eq!(other.previous, None);
    assert_eq!(id_of(&other, "Save"), "e_7", "IDs of the old session are never reused");
}

fn server_rows(names: &[&'static str]) -> Vec<Spec> {
    let mut specs = vec![window(), ax(Role::Table, "Servers", (0.0, 50.0, 600.0, 300.0), 0)];
    for (row, name) in names.iter().enumerate() {
        let y = 50.0 + 30.0 * row as f32;
        let parent = specs.len();
        specs.push(Spec {
            role: Role::Row,
            name: None,
            bounds: (0.0, y, 600.0, 30.0),
            parent: Some(1),
            source: Source::Accessibility,
        });
        specs.push(ax(Role::Text, name, (10.0, y + 5.0, 200.0, 20.0), parent));
        specs.push(ax(Role::Button, "Restart", (500.0, y + 3.0, 80.0, 24.0), parent));
    }
    specs
}

/// Scrolling a list by one row: rows and their identical buttons follow
/// their content, not the position.
#[test]
fn rows_follow_their_content_when_a_list_scrolls() {
    let mut tracker = Tracker::new();
    let first = track(&mut tracker, observe(1, (0.0, 0.0), &server_rows(&["A", "B", "C", "D"])));
    let second = track(&mut tracker, observe(2, (0.0, 0.0), &server_rows(&["B", "C", "D", "E"])));

    let row_of = |observation: &Observation, name: &str| {
        element(observation, name).parent.clone().expect("texts are in rows")
    };
    let restart_of = |observation: &Observation, name: &str| {
        let row = row_of(observation, name);
        observation
            .elements
            .iter()
            .find(|e| e.name.as_deref() == Some("Restart") && e.parent.as_ref() == Some(&row))
            .unwrap()
            .clone()
    };
    for name in ["B", "C", "D"] {
        assert_eq!(id_of(&first, name), id_of(&second, name), "{name}");
        assert_eq!(row_of(&first, name), row_of(&second, name), "row {name}");
        let (before, after) = (restart_of(&first, name), restart_of(&second, name));
        assert_eq!(before.id, after.id, "Restart in row {name}");
    }
    let ids_before: Vec<&ElementId> = first.elements.iter().map(|e| &e.id).collect();
    assert!(!ids_before.contains(&&element(&second, "E").id), "E is new");
    assert!(!ids_before.contains(&&row_of(&second, "E")));
}

/// Two identical unnamed boxes seen only in pixels, shifted so that each is
/// halfway between the old ones: the assignment is a guess and says so.
#[test]
fn ambiguous_identity_is_not_reported_as_certain() {
    let boxes = |offset: f32| -> Vec<Spec> {
        let pixel = |x: f32| Spec {
            role: Role::Unknown,
            name: None,
            bounds: (x, 100.0, 40.0, 40.0),
            parent: Some(0),
            source: Source::Vision,
        };
        vec![window(), pixel(100.0 + offset), pixel(140.0 + offset)]
    };
    let mut tracker = Tracker::new();
    track(&mut tracker, observe(1, (0.0, 0.0), &boxes(0.0)));
    let mut second = observe(2, (0.0, 0.0), &boxes(20.0));
    let report = tracker.track(&mut second, &[]);
    for element in &second.elements[1..] {
        if let Some(identity) = element.confidence.identity {
            assert!(identity.get() < UNCERTAIN, "{identity:?}");
        }
    }
    assert!(report.uncertain + report.new >= 2, "{report:?}");
}

/// Native identifiers unique in both observations identify elements whatever
/// else changed; a changed identifier alone is no evidence (applications
/// encode state in them).
#[test]
fn native_identifiers_decide() {
    let specs = vec![
        window(),
        ax(Role::Button, "Go", (10.0, 10.0, 60.0, 24.0), 0),
        ax(Role::Button, "Go", (100.0, 10.0, 60.0, 24.0), 0),
    ];
    let natives = |a: &str, b: &str| vec![None, Some(a.to_owned()), Some(b.to_owned())];
    let mut tracker = Tracker::new();
    let mut first = observe(1, (0.0, 0.0), &specs);
    tracker.track(&mut first, &natives("left", "right"));
    // The two buttons swapped places.
    let mut second = observe(2, (0.0, 0.0), &specs);
    tracker.track(&mut second, &natives("right", "left"));
    assert_eq!(first.elements[1].id, second.elements[2].id);
    assert_eq!(first.elements[2].id, second.elements[1].id);
    assert!(identity(&second.elements[1]) >= 0.9);

    // Calculator's display is `StandardInputView;value:57`, then `…:42`.
    let mut third = observe(3, (0.0, 0.0), &specs);
    tracker.track(&mut third, &natives("right", "left;value:42"));
    assert_eq!(third.elements[2].id, second.elements[2].id);
}

/// Empty structured twins (same place, same parent) are told apart by their
/// order among their siblings.
#[test]
fn identical_structured_twins_keep_their_order() {
    let twin = |source| Spec {
        role: Role::Group,
        name: None,
        bounds: (0.0, 50.0, 600.0, 300.0),
        parent: Some(0),
        source,
    };
    let mut specs = vec![window(), twin(Source::Accessibility)];
    specs.push(ax(Role::Group, "Content", (10.0, 60.0, 100.0, 100.0), 0));
    specs.push(twin(Source::Accessibility));
    let mut tracker = Tracker::new();
    let first = track(&mut tracker, observe(1, (0.0, 0.0), &specs));
    let second = track(&mut tracker, observe(2, (0.0, 0.0), &specs));
    for index in [1, 3] {
        assert_eq!(first.elements[index].id, second.elements[index].id);
        assert_eq!(identity(&second.elements[index]), 1.0);
    }
}

#[test]
fn relations_are_renamed() {
    let with_label = |number: u32, specs: &[Spec]| {
        let mut observation = observe(number, (0.0, 0.0), specs);
        let label = element(&observation, "Find").id.clone();
        let field = element(&observation, "Search").id.clone();
        observation.relations.push(Relation {
            kind: RelationKind::LabelFor,
            from: label,
            to: field,
            confidence: None,
        });
        observation
    };
    let mut specs = toolbar();
    specs.push(ax(Role::Text, "Find", (350.0, 12.0, 40.0, 16.0), 1));
    let mut tracker = Tracker::new();
    let first = track(&mut tracker, with_label(1, &specs));
    // A new button before the field shifts the fresh IDs assembly gives.
    specs.insert(3, ax(Role::Button, "Open", (80.0, 8.0, 60.0, 24.0), 1));
    let second = track(&mut tracker, with_label(2, &specs));
    assert_eq!(id_of(&first, "Search"), id_of(&second, "Search"));
    let relation = &second.relations[0];
    assert_eq!(relation.from, element(&second, "Find").id);
    assert_eq!(relation.to, element(&second, "Search").id);
}

/// The size limit of accessibility trees: 5,000 elements, with repeated
/// names, scrolled by one row. Run with
/// `cargo test --release -p argus-tracking -- --ignored --nocapture`.
#[test]
#[ignore = "timing; run in release"]
fn large_trees_are_tracked_quickly() {
    const NAMES: [&str; 8] = ["0", "1", "2", "3", "4", "5", "6", "7"];
    let table = |first: usize| {
        let mut specs = vec![window(), ax(Role::Table, "Big", (0.0, 0.0, 2000.0, 20000.0), 0)];
        for row in 0..1000 {
            let y = 20.0 * row as f32;
            let parent = specs.len();
            specs.push(Spec {
                role: Role::Row,
                name: None,
                bounds: (0.0, y, 2000.0, 20.0),
                parent: Some(1),
                source: Source::Accessibility,
            });
            let label: &'static str = Box::leak(format!("Item {}", first + row).into_boxed_str());
            specs.push(ax(Role::Text, label, (0.0, y, 200.0, 20.0), parent));
            for (column, name) in NAMES.iter().take(3).enumerate() {
                let x = 300.0 + 100.0 * column as f32;
                specs.push(ax(Role::Button, name, (x, y, 80.0, 20.0), parent));
            }
        }
        specs
    };
    let mut tracker = Tracker::new();
    let first = track(&mut tracker, observe(1, (0.0, 0.0), &table(0)));
    assert!(first.elements.len() > 5000);
    let started = std::time::Instant::now();
    let mut second = observe(2, (0.0, 0.0), &table(1));
    let report = tracker.track(&mut second, &[]);
    let elapsed = started.elapsed();
    eprintln!("{} elements tracked in {elapsed:?}: {report:?}", second.elements.len());
    assert_eq!((report.new, report.lost), (5, 5), "one row scrolled in, one out");
    assert!(elapsed.as_millis() < 1000);
}
