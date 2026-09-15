use super::*;
use crate::api::schema::{AgentMoveParams, SuccessResponse};

fn agent_move_server() -> HeadlessServer {
    let mut server = test_headless_server();
    server.app.state.workspaces = vec![
        crate::workspace::Workspace::test_new("one"),
        crate::workspace::Workspace::test_new("two"),
        crate::workspace::Workspace::test_new("three"),
    ];
    server.app.state.ensure_test_terminals();
    for ws_idx in 0..3 {
        let pane_id = server.app.state.workspaces[ws_idx].tabs[0].root_pane;
        let terminal_id = server.app.state.workspaces[ws_idx].tabs[0].panes[&pane_id]
            .attached_terminal_id
            .clone();
        let terminal = server.app.state.terminals.get_mut(&terminal_id).unwrap();
        terminal.set_agent_name(format!("agent-{ws_idx}"));
        terminal.set_detected_state(
            Some(crate::detect::Agent::Codex),
            crate::detect::AgentState::Idle,
        );
    }
    server.app.state.active = Some(0);
    server.app.state.selected = 0;
    server.app.state.mode = crate::app::Mode::Terminal;
    server
}

/// Moves one row and returns whether the server considered the frame dirty.
/// A reorder that does not report a render is invisible until some unrelated
/// event repaints, so callers assert on this.
fn move_agent(server: &mut HeadlessServer, pane_id: &str, before_pane_id: Option<&str>) -> bool {
    let (respond_to, response_rx) = std::sync::mpsc::channel();
    let changed = server.handle_api_request_with_shutdown_check(crate::api::ApiRequestMessage {
        request: crate::api::schema::Request {
            id: "move-agent".into(),
            method: crate::api::schema::Method::AgentMove(AgentMoveParams {
                pane_id: pane_id.into(),
                before_pane_id: before_pane_id.map(str::to_owned),
            }),
        },
        respond_to,
        response_write_complete: None,
        stream_active: None,
    });
    let response = response_rx.recv().expect("agent move response");
    serde_json::from_str::<SuccessResponse>(&response)
        .unwrap_or_else(|_| panic!("agent move failed: {response}"));
    changed
}

#[tokio::test]
async fn every_agent_move_reaches_attached_clients() {
    let mut server = agent_move_server();
    let (control_rx, render_rx) = connect_test_shell(&mut server, 9, 80, 23);
    let initial = client_shell_snapshot(read_server_message(control_rx.recv().unwrap()));
    let order = initial.agent_order.clone();
    assert_eq!(order.len(), 3, "three agent rows: {order:?}");

    // The render channel is bounded, so drain it like a live client does;
    // otherwise later surfaces come back Full and are merely deferred.
    let mut surface_revisions = Vec::new();
    let drain = |rx: &std::sync::mpsc::Receiver<Vec<u8>>, out: &mut Vec<u64>| {
        while let Ok(message) = rx.try_recv() {
            match read_server_message(message) {
                ServerMessage::PaneSurface(surface) => out.push(surface.projection_revision),
                ServerMessage::PaneSurfacePatch(patch) => out.push(patch.projection_revision),
                _ => {}
            }
        }
    };
    drain(&render_rx, &mut surface_revisions);

    // First move: the panel leaves its derived order.
    assert!(
        move_agent(&mut server, &order[0], None),
        "a reorder must mark the frame dirty"
    );
    server.render_and_stream();
    drain(&render_rx, &mut surface_revisions);
    let first = client_shell_snapshot(read_server_message(control_rx.recv().unwrap()));
    assert!(first.revision > initial.revision);
    assert_eq!(
        first.agent_order,
        vec![order[1].clone(), order[2].clone(), order[0].clone()]
    );

    // Second move: already user-ordered, so only the order itself changes.
    assert!(
        move_agent(&mut server, &order[0], Some(&order[1])),
        "a later reorder must still mark the frame dirty"
    );
    server.render_and_stream();
    drain(&render_rx, &mut surface_revisions);
    let second = client_shell_snapshot(read_server_message(control_rx.recv().unwrap()));
    assert!(
        second.revision > first.revision,
        "a later reorder must still publish a new snapshot"
    );
    assert_eq!(
        second.agent_order,
        vec![order[0].clone(), order[1].clone(), order[2].clone()],
        "the second reorder must reach attached clients"
    );

    // The client cannot compose a snapshot until a pane surface carrying the
    // same projection revision arrives, so the reorder must publish one.
    assert!(
        surface_revisions.contains(&second.revision),
        "no pane surface published for revision {}; surfaces seen: {surface_revisions:?}",
        second.revision
    );
}
