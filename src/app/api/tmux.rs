use std::path::PathBuf;

use crate::api::schema::{
    EventData, EventEnvelope, EventKind, ResponseResult, TmuxAttachParams, TmuxSessionInfo,
};
use crate::app::App;
use crate::tmux::TmuxSession;

use super::responses::{encode_error, encode_success};

impl App {
    pub(crate) fn workspace_tmux_session(&self, ws_idx: usize) -> Option<String> {
        let ws = self.state.workspaces.get(ws_idx)?;
        let tab = ws.tabs.first()?;
        let pane = tab.panes.get(&tab.root_pane)?;
        let terminal = self.state.terminals.get(&pane.attached_terminal_id)?;
        let argv = terminal.launch_argv.as_deref()?;
        crate::tmux::session_name_from_launch_argv(argv).map(str::to_string)
    }

    fn open_workspace_idx_for_tmux_session(&self, session: &str) -> Option<usize> {
        (0..self.state.workspaces.len())
            .find(|ws_idx| self.workspace_tmux_session(*ws_idx).as_deref() == Some(session))
    }

    fn tmux_session_info(&self, session: &TmuxSession) -> TmuxSessionInfo {
        TmuxSessionInfo {
            name: session.name.clone(),
            windows: session.windows,
            attached: session.attached,
            created_unix: session.created_unix,
            path: session.path.clone(),
            open_workspace_id: self
                .open_workspace_idx_for_tmux_session(&session.name)
                .map(|ws_idx| self.public_workspace_id(ws_idx)),
        }
    }

    pub(super) fn handle_tmux_list(&mut self, id: String) -> String {
        match crate::tmux::list_sessions() {
            Ok(sessions) => encode_success(
                id,
                ResponseResult::TmuxSessionList {
                    sessions: sessions
                        .iter()
                        .map(|session| self.tmux_session_info(session))
                        .collect(),
                },
            ),
            Err(err) => encode_error(id, err.code(), err.message()),
        }
    }

    pub(super) fn handle_tmux_attach(&mut self, id: String, params: TmuxAttachParams) -> String {
        if let Err(message) = crate::tmux::validate_session_name(&params.session) {
            return encode_error(id, "invalid_request", message);
        }
        let session = match crate::tmux::list_sessions() {
            Ok(sessions) => sessions
                .into_iter()
                .find(|session| session.name == params.session),
            Err(err) => return encode_error(id, err.code(), err.message()),
        };
        let Some(session) = session else {
            return encode_error(
                id,
                "tmux_session_not_found",
                format!("tmux session {} not found", params.session),
            );
        };
        self.attach_tmux_session(id, params, session)
    }

    pub(super) fn attach_tmux_session(
        &mut self,
        id: String,
        params: TmuxAttachParams,
        session: TmuxSession,
    ) -> String {
        let extra_env = match super::env::normalize_launch_env(params.env) {
            Ok(env) => env,
            Err((code, message)) => return encode_error(id, &code, message),
        };
        let already_open = self.open_workspace_idx_for_tmux_session(&session.name);
        let (ws_idx, created_workspace) = if let Some(ws_idx) = already_open {
            if params.focus {
                self.state.switch_workspace(ws_idx);
            }
            (ws_idx, false)
        } else {
            let cwd = params
                .cwd
                .map(PathBuf::from)
                .or_else(|| session.path.as_deref().map(PathBuf::from))
                .filter(|path| path.is_dir())
                .unwrap_or_else(|| self.resolve_new_terminal_cwd(None));
            let argv = crate::tmux::attach_argv(&session.name);
            match self.create_workspace_with_argv(cwd, params.focus, &argv, extra_env) {
                Ok(ws_idx) => (ws_idx, true),
                Err(err) => return encode_error(id, "tmux_attach_failed", err.to_string()),
            }
        };

        let label = match (params.label, created_workspace) {
            (Some(label), _) => Some(label),
            (None, true) => Some(crate::tmux::default_label(&session.name)),
            (None, false) => None,
        };
        if let Some(label) = label {
            let workspace_id = self.public_workspace_id(ws_idx);
            if let Some(ws) = self.state.workspaces.get_mut(ws_idx) {
                ws.set_custom_name(label.clone());
                crate::logging::workspace_renamed(&ws.id);
            }
            if !created_workspace {
                self.emit_event(EventEnvelope {
                    event: EventKind::WorkspaceRenamed,
                    data: EventData::WorkspaceRenamed {
                        workspace_id,
                        label,
                    },
                });
            }
        }
        self.state.mark_session_dirty();
        if created_workspace {
            self.emit_workspace_open_events(ws_idx);
        }

        let tab_idx = self.state.workspaces[ws_idx].active_tab;
        let session = self.tmux_session_info(&session);
        encode_success(
            id,
            ResponseResult::TmuxAttached {
                workspace: self.workspace_info(ws_idx),
                tab: self
                    .tab_info(ws_idx, tab_idx)
                    .expect("attached tmux workspace should have an active tab"),
                root_pane: self
                    .root_pane_info(ws_idx, tab_idx)
                    .expect("attached tmux workspace should have an active root pane"),
                session,
                already_open: already_open.is_some(),
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::schema::SuccessResponse;
    use crate::app::api::test_support::{exiting_test_command, shutdown_test_runtimes};
    use crate::config::{Config, ShellModeConfig};
    use crate::workspace::Workspace;

    fn test_app() -> App {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            &Config::default(),
            crate::app::AppPolicy::TEST,
            None,
            api_rx,
            crate::api::EventHub::default(),
        );
        app.state.default_shell = exiting_test_command().into();
        app.state.shell_mode = ShellModeConfig::NonLogin;
        app.state.workspaces = vec![Workspace::test_new("first")];
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.ensure_test_terminals();
        shutdown_test_runtimes(&mut app);
        app
    }

    fn params(session: &str, focus: bool) -> TmuxAttachParams {
        TmuxAttachParams {
            session: session.into(),
            focus,
            label: None,
            cwd: None,
            env: Default::default(),
        }
    }

    fn session(name: &str) -> TmuxSession {
        TmuxSession {
            name: name.into(),
            windows: 2,
            attached: 0,
            created_unix: Some(1),
            path: Some(std::env::temp_dir().display().to_string()),
        }
    }

    #[tokio::test]
    async fn attach_creates_workspace_with_tmux_launch_argv_and_default_label() {
        let mut app = test_app();

        let response = app.attach_tmux_session("req".into(), params("api", false), session("api"));

        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        let ResponseResult::TmuxAttached {
            workspace,
            session,
            already_open,
            ..
        } = success.result
        else {
            panic!("expected tmux attached response");
        };
        assert!(!already_open);
        assert_eq!(workspace.label, "tmux:api");
        assert_eq!(workspace.tmux_session.as_deref(), Some("api"));
        assert_eq!(
            session.open_workspace_id.as_deref(),
            Some(workspace.workspace_id.as_str())
        );
        assert_eq!(app.state.workspaces.len(), 2);
        assert_eq!(app.workspace_tmux_session(1).as_deref(), Some("api"));
        let root = app.state.workspaces[1].tabs[0].root_pane;
        let terminal_id = app.state.workspaces[1].terminal_id(root).cloned().unwrap();
        let terminal = &app.state.terminals[&terminal_id];
        assert_eq!(
            terminal.launch_argv.as_deref(),
            Some(crate::tmux::attach_argv("api").as_slice())
        );
        assert!(terminal.respawn_shell_on_exit);
        assert_eq!(app.state.active, Some(0), "no-focus attach keeps focus");
        shutdown_test_runtimes(&mut app);
    }

    #[tokio::test]
    async fn attach_reuses_open_workspace_for_same_session() {
        let mut app = test_app();
        let first = app.attach_tmux_session("a".into(), params("api", false), session("api"));
        let first: SuccessResponse = serde_json::from_str(&first).unwrap();
        let ResponseResult::TmuxAttached { workspace, .. } = first.result else {
            panic!("expected tmux attached response");
        };

        let second = app.attach_tmux_session("b".into(), params("api", true), session("api"));

        let second: SuccessResponse = serde_json::from_str(&second).unwrap();
        let ResponseResult::TmuxAttached {
            workspace: reused,
            already_open,
            ..
        } = second.result
        else {
            panic!("expected tmux attached response");
        };
        assert!(already_open);
        assert_eq!(reused.workspace_id, workspace.workspace_id);
        assert_eq!(app.state.workspaces.len(), 2);
        assert_eq!(app.state.active, Some(1), "focus attach switches workspace");
        shutdown_test_runtimes(&mut app);
    }

    #[tokio::test]
    async fn attach_honors_custom_label_and_cwd() {
        let mut app = test_app();
        let cwd = std::env::temp_dir().join(format!("herdr-tmux-cwd-{}", std::process::id()));
        std::fs::create_dir_all(&cwd).unwrap();
        let mut params = params("batch", false);
        params.label = Some("jobs".into());
        params.cwd = Some(cwd.display().to_string());

        let response = app.attach_tmux_session("req".into(), params, session("batch"));

        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        let ResponseResult::TmuxAttached { workspace, .. } = success.result else {
            panic!("expected tmux attached response");
        };
        assert_eq!(workspace.label, "jobs");
        assert_eq!(
            crate::worktree::canonical_or_original(&app.state.workspaces[1].identity_cwd),
            crate::worktree::canonical_or_original(&cwd)
        );
        shutdown_test_runtimes(&mut app);
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn handle_tmux_attach_rejects_invalid_session_names() {
        let mut app = test_app();
        let response = app.handle_tmux_attach("req".into(), params("a:b", false));
        let value: serde_json::Value = serde_json::from_str(&response).unwrap();
        assert_eq!(value["error"]["code"], "invalid_request");
        assert_eq!(app.state.workspaces.len(), 1);
    }
}
