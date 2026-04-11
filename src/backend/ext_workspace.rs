use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use gpui::{AsyncApp, Task};
use wayland_client::{
    globals::{registry_queue_init, GlobalListContents},
    protocol::wl_registry,
    Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum,
};
use wayland_protocols::ext::workspace::v1::client::{
    ext_workspace_group_handle_v1::{self, ExtWorkspaceGroupHandleV1},
    ext_workspace_handle_v1::{self, ExtWorkspaceHandleV1, State as WsState},
    ext_workspace_manager_v1::{self, ExtWorkspaceManagerV1},
};

use crate::backend::{
    EventSink, Workspace, WorkspaceBackend, WorkspaceEvent, WorkspaceId, WorkspaceState,
};

#[derive(Default, Clone)]
struct WsBuilder {
    name: Option<String>,
    active: bool,
    urgent: bool,
}

struct AppState {
    workspaces: HashMap<u32, (ExtWorkspaceHandleV1, WsBuilder)>,
    by_name: HashMap<String, ExtWorkspaceHandleV1>,
    sink: EventSink,
    activate_request: Arc<Mutex<Option<WorkspaceId>>>,
}

impl AppState {
    fn flush_state(&mut self) {
        self.by_name.clear();
        let mut workspaces: Vec<Workspace> = Vec::new();
        for (handle, b) in self.workspaces.values() {
            let Some(name) = b.name.as_ref().cloned() else {
                continue;
            };
            self.by_name.insert(name.clone(), handle.clone());
            workspaces.push(Workspace {
                id: WorkspaceId(name.clone()),
                name,
                active: b.active,
                urgent: b.urgent,
            });
        }
        workspaces.sort_by(|a, b| a.name.cmp(&b.name));
        let active = workspaces.iter().find(|w| w.active).map(|w| w.id.clone());
        let _ = self
            .sink
            .send_blocking(WorkspaceEvent::Snapshot(WorkspaceState {
                workspaces,
                active,
            }));
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for AppState {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ExtWorkspaceManagerV1, ()> for AppState {
    fn event(
        state: &mut Self,
        _: &ExtWorkspaceManagerV1,
        event: ext_workspace_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_workspace_manager_v1::Event::Workspace { workspace } => {
                let id = workspace.id().protocol_id();
                state
                    .workspaces
                    .insert(id, (workspace, WsBuilder::default()));
            }
            ext_workspace_manager_v1::Event::Done => {
                state.flush_state();
            }
            ext_workspace_manager_v1::Event::WorkspaceGroup { .. } => {}
            ext_workspace_manager_v1::Event::Finished => {
                log::info!("ext-workspace manager finished");
            }
            _ => {}
        }
    }
}

impl Dispatch<ExtWorkspaceGroupHandleV1, ()> for AppState {
    fn event(
        _: &mut Self,
        _: &ExtWorkspaceGroupHandleV1,
        _event: ext_workspace_group_handle_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ExtWorkspaceHandleV1, ()> for AppState {
    fn event(
        state: &mut Self,
        handle: &ExtWorkspaceHandleV1,
        event: ext_workspace_handle_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let id = handle.id().protocol_id();
        match event {
            ext_workspace_handle_v1::Event::Name { name } => {
                if let Some((_, builder)) = state.workspaces.get_mut(&id) {
                    builder.name = Some(name);
                }
            }
            ext_workspace_handle_v1::Event::Coordinates { .. } => {
                // Ignored: we don't arrange workspaces geometrically.
            }
            ext_workspace_handle_v1::Event::State { state: s } => {
                if let Some((_, builder)) = state.workspaces.get_mut(&id) {
                    let bits: WsState = match s {
                        WEnum::Value(v) => v,
                        WEnum::Unknown(_) => WsState::empty(),
                    };
                    builder.active = bits.contains(WsState::Active);
                    builder.urgent = bits.contains(WsState::Urgent);
                }
            }
            ext_workspace_handle_v1::Event::Removed => {
                state.workspaces.remove(&id);
            }
            _ => {}
        }
    }
}

pub struct ExtWorkspaceBackend {
    activate_request: Arc<Mutex<Option<WorkspaceId>>>,
}

impl ExtWorkspaceBackend {
    pub fn new() -> Self {
        ExtWorkspaceBackend {
            activate_request: Arc::new(Mutex::new(None)),
        }
    }

    /// Check whether the running compositor advertises ext_workspace_manager_v1.
    pub fn probe() -> bool {
        let Ok(conn) = Connection::connect_to_env() else {
            return false;
        };
        let Ok((globals, _queue)) = registry_queue_init::<ProbeState>(&conn) else {
            return false;
        };
        globals.contents().with_list(|list| {
            list.iter()
                .any(|g| g.interface == "ext_workspace_manager_v1")
        })
    }
}

struct ProbeState;

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for ProbeState {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl WorkspaceBackend for ExtWorkspaceBackend {
    fn run(&self, sink: EventSink, cx: &mut AsyncApp) -> Task<()> {
        let activate_request = self.activate_request.clone();
        cx.background_executor().spawn(async move {
            if let Err(e) = run_session(sink.clone(), activate_request) {
                log::warn!("ext-workspace session error: {e:#}");
            }
            let _ = sink.send_blocking(WorkspaceEvent::Disconnected);
        })
    }

    fn activate(&self, id: &WorkspaceId) {
        *self.activate_request.lock().unwrap() = Some(id.clone());
    }
}

fn run_session(
    sink: EventSink,
    activate_request: Arc<Mutex<Option<WorkspaceId>>>,
) -> anyhow::Result<()> {
    let conn = Connection::connect_to_env()?;
    let (globals, mut event_queue): (_, EventQueue<AppState>) = registry_queue_init(&conn)?;
    let qh = event_queue.handle();

    let manager: ExtWorkspaceManagerV1 = globals
        .bind(&qh, 1..=1, ())
        .map_err(|e| anyhow::anyhow!("failed to bind ext_workspace_manager_v1: {e}"))?;

    let mut state = AppState {
        workspaces: HashMap::new(),
        by_name: HashMap::new(),
        sink,
        activate_request,
    };

    loop {
        event_queue.blocking_dispatch(&mut state)?;

        // Drain pending activation requests after each dispatch.
        let pending = state.activate_request.lock().unwrap().take();
        if let Some(req) = pending {
            if let Some(handle) = state.by_name.get(&req.0) {
                handle.activate();
                manager.commit();
                conn.flush()?;
            } else {
                log::warn!("activate: workspace '{}' not found", req.0);
            }
        }
    }
}
