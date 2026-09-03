use std::process::Command;

const TMUX_PROGRAM: &str = "tmux";
const ATTACH_SUBCOMMAND: &str = "attach-session";

const LIST_SESSIONS_FORMAT: &str =
    "#{session_name}\t#{session_windows}\t#{session_attached}\t#{session_created}\t#{session_path}";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TmuxSession {
    pub name: String,
    pub windows: usize,
    pub attached: usize,
    pub created_unix: Option<u64>,
    pub path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TmuxError {
    Unavailable(String),
    Failed(String),
}

impl TmuxError {
    pub fn code(&self) -> &'static str {
        match self {
            TmuxError::Unavailable(_) => "tmux_unavailable",
            TmuxError::Failed(_) => "tmux_failed",
        }
    }

    pub fn message(&self) -> String {
        match self {
            TmuxError::Unavailable(detail) => format!("tmux is not available: {detail}"),
            TmuxError::Failed(detail) => format!("tmux command failed: {detail}"),
        }
    }
}

pub fn validate_session_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("tmux session name must not be empty".into());
    }
    if name.contains('\0') {
        return Err("tmux session name must not contain NUL bytes".into());
    }
    if name.contains(':') || name.contains('.') {
        return Err("tmux session name must not contain ':' or '.'".into());
    }
    if name.chars().any(char::is_whitespace) {
        return Err("tmux session name must not contain whitespace".into());
    }
    Ok(())
}

// The '=' prefix forces an exact name match instead of tmux's prefix match.
pub fn attach_argv(session: &str) -> Vec<String> {
    vec![
        TMUX_PROGRAM.to_string(),
        ATTACH_SUBCOMMAND.to_string(),
        "-t".to_string(),
        format!("={session}"),
    ]
}

pub fn session_name_from_launch_argv(argv: &[String]) -> Option<&str> {
    match argv {
        [program, subcommand, flag, target]
            if program == TMUX_PROGRAM && subcommand == ATTACH_SUBCOMMAND && flag == "-t" =>
        {
            target.strip_prefix('=').filter(|name| !name.is_empty())
        }
        _ => None,
    }
}

pub fn default_label(session: &str) -> String {
    format!("tmux:{session}")
}

pub fn parse_list_sessions(output: &str) -> Vec<TmuxSession> {
    output
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let name = fields.next()?.trim();
            if name.is_empty() {
                return None;
            }
            let windows = fields
                .next()
                .and_then(|value| value.trim().parse().ok())
                .unwrap_or(0);
            let attached = fields
                .next()
                .and_then(|value| value.trim().parse().ok())
                .unwrap_or(0);
            let created_unix = fields.next().and_then(|value| value.trim().parse().ok());
            let path = fields
                .next()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string);
            Some(TmuxSession {
                name: name.to_string(),
                windows,
                attached,
                created_unix,
                path,
            })
        })
        .collect()
}

fn no_server_output(stderr: &str) -> bool {
    let stderr = stderr.trim();
    stderr.contains("no server running")
        || stderr.contains("no sessions")
        || stderr.contains("error connecting to")
}

pub fn list_sessions() -> Result<Vec<TmuxSession>, TmuxError> {
    let output = Command::new(TMUX_PROGRAM)
        .args(["list-sessions", "-F", LIST_SESSIONS_FORMAT])
        .output()
        .map_err(|err| TmuxError::Unavailable(err.to_string()))?;
    if output.status.success() {
        return Ok(parse_list_sessions(&String::from_utf8_lossy(
            &output.stdout,
        )));
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    if no_server_output(&stderr) {
        return Ok(Vec::new());
    }
    Err(TmuxError::Failed(stderr.trim().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attach_argv_uses_exact_match_target() {
        assert_eq!(
            attach_argv("api"),
            vec!["tmux", "attach-session", "-t", "=api"]
        );
    }

    #[test]
    fn session_name_round_trips_through_attach_argv() {
        let argv = attach_argv("work-1");
        assert_eq!(session_name_from_launch_argv(&argv), Some("work-1"));
    }

    #[test]
    fn session_name_rejects_unrelated_argv() {
        let cases: [&[&str]; 5] = [
            &[],
            &["tmux"],
            &["tmux", "new-session", "-t", "=api"],
            &["tmux", "attach-session", "-t", "api"],
            &["just", "dev"],
        ];
        for case in cases {
            let argv: Vec<String> = case.iter().map(|arg| arg.to_string()).collect();
            assert_eq!(session_name_from_launch_argv(&argv), None, "{case:?}");
        }
    }

    #[test]
    fn validate_session_name_rejects_target_separators() {
        assert!(validate_session_name("api").is_ok());
        assert!(validate_session_name("api-2_x").is_ok());
        assert!(validate_session_name("").is_err());
        assert!(validate_session_name("a:b").is_err());
        assert!(validate_session_name("a.b").is_err());
        assert!(validate_session_name("a b").is_err());
        assert!(validate_session_name("a\0b").is_err());
    }

    #[test]
    fn parse_list_sessions_reads_all_fields() {
        let output = "api\t3\t1\t1725000000\t/home/me/api\nbatch\t1\t0\t1725000100\t\n";
        let sessions = parse_list_sessions(output);
        assert_eq!(
            sessions,
            vec![
                TmuxSession {
                    name: "api".into(),
                    windows: 3,
                    attached: 1,
                    created_unix: Some(1_725_000_000),
                    path: Some("/home/me/api".into()),
                },
                TmuxSession {
                    name: "batch".into(),
                    windows: 1,
                    attached: 0,
                    created_unix: Some(1_725_000_100),
                    path: None,
                },
            ]
        );
    }

    #[test]
    fn parse_list_sessions_tolerates_short_lines_and_blank_lines() {
        let sessions = parse_list_sessions("\nonly-name\n\nother\t2\n");
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].name, "only-name");
        assert_eq!(sessions[0].windows, 0);
        assert_eq!(sessions[1].name, "other");
        assert_eq!(sessions[1].windows, 2);
        assert_eq!(sessions[1].created_unix, None);
    }

    #[test]
    fn no_server_output_matches_tmux_messages() {
        assert!(no_server_output(
            "no server running on /tmp/tmux-1000/default\n"
        ));
        assert!(no_server_output(
            "error connecting to /tmp/tmux-1000/default (No such file or directory)"
        ));
        assert!(!no_server_output("unknown option -- Z"));
    }
}
