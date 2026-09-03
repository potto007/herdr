use std::collections::HashMap;

use crate::api::schema::TmuxAttachParams;

pub(super) fn run_tmux_command(args: &[String]) -> std::io::Result<i32> {
    let Some(subcommand) = args.first().map(|arg| arg.as_str()) else {
        print_tmux_help();
        return Ok(2);
    };

    match subcommand {
        "list" => tmux_list(&args[1..]),
        "attach" => tmux_attach(&args[1..]),
        "help" | "--help" | "-h" => {
            print_tmux_help();
            Ok(0)
        }
        _ => {
            print_tmux_help();
            Ok(2)
        }
    }
}

fn tmux_list(args: &[String]) -> std::io::Result<i32> {
    if !args.is_empty() {
        eprintln!("usage: herdr tmux list");
        return Ok(2);
    }
    super::runtime::tmux_list()
}

fn tmux_attach(args: &[String]) -> std::io::Result<i32> {
    let mut session = None;
    let mut label = None;
    let mut cwd = None;
    let mut focus = false;
    let mut env = HashMap::new();

    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--label" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --label");
                    return Ok(2);
                };
                label = Some(value.clone());
                index += 2;
            }
            "--cwd" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --cwd");
                    return Ok(2);
                };
                cwd = Some(normalize_path_arg(value)?);
                index += 2;
            }
            "--env" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --env");
                    return Ok(2);
                };
                let (key, value) = match super::parse_env_assignment(value) {
                    Ok(pair) => pair,
                    Err(err) => {
                        eprintln!("{err}");
                        return Ok(2);
                    }
                };
                env.insert(key, value);
                index += 2;
            }
            "--focus" => {
                focus = true;
                index += 1;
            }
            "--no-focus" => {
                focus = false;
                index += 1;
            }
            other if other.starts_with('-') => {
                eprintln!("unknown option: {other}");
                return Ok(2);
            }
            other => {
                if session.is_some() {
                    eprintln!("{TMUX_ATTACH_USAGE}");
                    return Ok(2);
                }
                session = Some(other.to_string());
                index += 1;
            }
        }
    }

    let Some(session) = session else {
        eprintln!("{TMUX_ATTACH_USAGE}");
        return Ok(2);
    };
    if let Err(message) = crate::tmux::validate_session_name(&session) {
        eprintln!("{message}");
        return Ok(2);
    }

    super::runtime::tmux_attach(TmuxAttachParams {
        session,
        focus,
        label,
        cwd,
        env,
    })
}

const TMUX_ATTACH_USAGE: &str =
    "usage: herdr tmux attach <session> [--label TEXT] [--cwd PATH] [--env KEY=VALUE] [--focus] [--no-focus]";

fn print_tmux_help() {
    eprintln!("herdr tmux commands:");
    eprintln!("  herdr tmux list");
    eprintln!("  {TMUX_ATTACH_USAGE}");
}

fn normalize_path_arg(value: &str) -> std::io::Result<String> {
    let path = crate::worktree::expand_tilde_path(value);
    let absolute = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()?.join(path)
    };
    Ok(absolute.display().to_string())
}
