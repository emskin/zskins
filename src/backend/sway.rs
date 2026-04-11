use serde::Deserialize;
use anyhow::Result;
use crate::backend::{Workspace, WorkspaceId, WorkspaceState};

#[derive(Deserialize)]
struct RawWorkspace {
    name: String,
    focused: bool,
    urgent: bool,
}

pub fn parse_get_workspaces(raw: &str) -> Result<WorkspaceState> {
    let raws: Vec<RawWorkspace> = serde_json::from_str(raw)?;
    let mut active = None;
    let workspaces: Vec<Workspace> = raws.into_iter().map(|r| {
        let id = WorkspaceId(r.name.clone());
        if r.focused { active = Some(id.clone()); }
        Workspace {
            id,
            name: r.name,
            active: r.focused,
            urgent: r.urgent,
        }
    }).collect();
    Ok(WorkspaceState { workspaces, active })
}

pub struct WorkspaceChange {
    pub new_active: Option<WorkspaceId>,
}

#[derive(Deserialize)]
struct RawEvent {
    change: String,
    current: Option<RawWorkspace>,
}

pub fn parse_workspace_event(raw: &str) -> Result<WorkspaceChange> {
    let ev: RawEvent = serde_json::from_str(raw)?;
    let new_active = if ev.change == "focus" {
        ev.current.map(|w| WorkspaceId(w.name))
    } else {
        None
    };
    Ok(WorkspaceChange { new_active })
}
