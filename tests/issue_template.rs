//! The bug report template's fields the app relies on: Report an Issue…
//! opens it with `version` filled in from the URL, and tells the user to
//! paste the system report into `system-info`.

/// One of the template's `body` items.
#[derive(Debug, Default)]
struct Item {
    kind: String,
    id: Option<String>,
    required: bool,
    render: Option<String>,
}

/// The `body` items of an issue form, read from its YAML layout: each item
/// starts with `  - type:` and its keys are indented beneath it.
fn items(yaml: &str) -> Vec<Item> {
    let body = yaml
        .split_once("\nbody:\n")
        .expect("an issue form has a body")
        .1;
    let mut items: Vec<Item> = Vec::new();
    for line in body.lines() {
        if let Some(kind) = line.strip_prefix("  - type: ") {
            items.push(Item {
                kind: kind.trim().to_string(),
                ..Default::default()
            });
            continue;
        }
        let Some(item) = items.last_mut() else {
            continue;
        };
        let value = |key: &str| line.strip_prefix(key).map(|v| v.trim().to_string());
        if let Some(id) = value("    id: ") {
            item.id = Some(id);
        } else if let Some(render) = value("      render: ") {
            item.render = Some(render);
        } else if value("      required: ").as_deref() == Some("true") {
            item.required = true;
        }
    }
    items
}

#[test]
fn the_bug_report_has_the_fields_the_app_fills_in() {
    let yaml = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/.github/ISSUE_TEMPLATE/bug_report.yml"
    ))
    .unwrap();
    let items = items(&yaml);
    let find = |id: &str| {
        let found: Vec<_> = items
            .iter()
            .filter(|i| i.id.as_deref() == Some(id))
            .collect();
        assert_eq!(found.len(), 1, "one {id} field");
        found[0]
    };
    // The new issue URL fills it in, so its id must not change.
    let version = find("version");
    assert_eq!(version.kind, "input");
    assert!(version.required);
    let system = find("system-info");
    assert_eq!(system.kind, "textarea");
    assert_eq!(system.render.as_deref(), Some("text"));
    // Optional: older versions can't produce it.
    assert!(!system.required);
    // Near the top: right after the checklist.
    let position = |id: &str| items.iter().position(|i| i.id.as_deref() == Some(id));
    assert_eq!(
        position("system-info"),
        position("checklist").map(|n| n + 1)
    );
    // Every existing field is kept until a release ships the dialog.
    for id in [
        "checklist",
        "installation",
        "operating-system",
        "system-version",
        "reproduction",
        "actual",
        "expected",
        "frequency",
        "camera-and-raw",
        "settings-and-hardware",
        "attachments",
        "diagnostics",
        "additional-context",
    ] {
        find(id);
    }
}
