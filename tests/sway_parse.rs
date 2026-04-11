use zbar::backend::sway::{parse_get_workspaces, parse_workspace_event};
use zbar::backend::{Workspace, WorkspaceId};

#[test]
fn parses_get_workspaces_into_state() {
    let raw = include_str!("fixtures/sway_workspaces.json");
    let state = parse_get_workspaces(raw).unwrap();

    assert_eq!(state.workspaces.len(), 3);
    assert_eq!(state.active, Some(WorkspaceId("1".to_string())));
    assert_eq!(state.workspaces[0], Workspace {
        id: WorkspaceId("1".to_string()),
        name: "1".to_string(),
        active: true,
        urgent: false,
    });
    assert_eq!(state.workspaces[2].urgent, true);
    assert_eq!(state.workspaces[2].name, "3:web".to_string());
}

#[test]
fn parses_workspace_focus_event_into_state_change() {
    let raw = include_str!("fixtures/sway_workspace_event.json");
    let change = parse_workspace_event(raw).unwrap();
    assert_eq!(change.new_active, Some(WorkspaceId("2".to_string())));
}
