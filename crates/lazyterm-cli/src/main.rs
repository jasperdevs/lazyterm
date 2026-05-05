use std::fmt;
use std::path::PathBuf;
use std::process::ExitCode;

use lazyterm_api::ApiRequest;
use lazyterm_core::AgentKind;

fn main() -> ExitCode {
    match parse_cli(std::env::args().skip(1)) {
        Ok(ParsedCli::Help(topic)) => {
            println!("{}", help_text(topic));
            ExitCode::SUCCESS
        }
        Ok(ParsedCli::Request(request)) => {
            print_request(&request);
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("error: {}", err.message);
            eprintln!();
            eprintln!("{}", help_text(err.help));
            ExitCode::from(1)
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HelpTopic {
    General,
    List,
    Status,
    New,
    Run,
    Focus,
}

#[derive(Debug, Eq, PartialEq)]
enum ParsedCli {
    Help(HelpTopic),
    Request(ApiRequest),
}

#[derive(Debug, Eq, PartialEq)]
struct CliError {
    message: String,
    help: HelpTopic,
}

impl CliError {
    fn new(message: impl Into<String>, help: HelpTopic) -> Self {
        Self {
            message: message.into(),
            help,
        }
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

fn parse_cli(mut args: impl Iterator<Item = String>) -> Result<ParsedCli, CliError> {
    match args.next() {
        None => Ok(ParsedCli::Help(HelpTopic::General)),
        Some(command) if is_help_flag(&command) => Ok(ParsedCli::Help(HelpTopic::General)),
        Some(command) => match command.as_str() {
            "help" => parse_help_command(args),
            "list" => parse_no_arg_command(args, HelpTopic::List, ApiRequest::ListSessions),
            "status" => parse_no_arg_command(args, HelpTopic::Status, ApiRequest::Status),
            "new" => parse_session_command(args, AgentKind::Shell, HelpTopic::New),
            "run" => parse_session_command(args, AgentKind::Codex, HelpTopic::Run),
            "focus" => parse_focus_command(args),
            _ => Err(CliError::new(
                format!("unknown command '{command}'"),
                HelpTopic::General,
            )),
        },
    }
}

fn parse_help_command(mut args: impl Iterator<Item = String>) -> Result<ParsedCli, CliError> {
    match args.next() {
        None => Ok(ParsedCli::Help(HelpTopic::General)),
        Some(topic) if is_help_flag(&topic) => Ok(ParsedCli::Help(HelpTopic::General)),
        Some(topic) => {
            if args.next().is_some() {
                return Err(CliError::new(
                    "help takes at most one topic",
                    HelpTopic::General,
                ));
            }

            match topic.as_str() {
                "list" => Ok(ParsedCli::Help(HelpTopic::List)),
                "status" => Ok(ParsedCli::Help(HelpTopic::Status)),
                "new" => Ok(ParsedCli::Help(HelpTopic::New)),
                "run" => Ok(ParsedCli::Help(HelpTopic::Run)),
                "focus" => Ok(ParsedCli::Help(HelpTopic::Focus)),
                "help" => Ok(ParsedCli::Help(HelpTopic::General)),
                _ => Err(CliError::new(
                    format!("unknown help topic '{topic}'"),
                    HelpTopic::General,
                )),
            }
        }
    }
}

fn parse_no_arg_command(
    mut args: impl Iterator<Item = String>,
    help: HelpTopic,
    request: ApiRequest,
) -> Result<ParsedCli, CliError> {
    if let Some(arg) = args.next() {
        if is_help_flag(&arg) {
            return Ok(ParsedCli::Help(help));
        }

        return Err(CliError::new(format!("unexpected argument '{arg}'"), help));
    }

    Ok(ParsedCli::Request(request))
}

fn parse_session_command(
    args: impl Iterator<Item = String>,
    default_agent: AgentKind,
    help: HelpTopic,
) -> Result<ParsedCli, CliError> {
    let mut cwd = None;
    let mut agent = default_agent;
    let mut task = None;
    let mut positional_task = Vec::new();
    let mut args = args.peekable();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(ParsedCli::Help(help)),
            "--cwd" | "-C" => {
                cwd = Some(PathBuf::from(next_value(&mut args, "--cwd", help)?));
            }
            "--agent" | "-a" => {
                agent = parse_agent_kind(&next_value(&mut args, "--agent", help)?, help)?;
            }
            "--task" | "-t" => {
                task = Some(collect_task_text(args, help, "--task")?);
                break;
            }
            "--" => {
                task = Some(collect_task_text(args, help, "task text")?);
                break;
            }
            _ if arg.starts_with('-') => {
                return Err(CliError::new(format!("unknown option '{arg}'"), help));
            }
            _ => positional_task.push(arg),
        }
    }

    if task.is_none() && !positional_task.is_empty() {
        task = Some(positional_task.join(" "));
    }

    let request = ApiRequest::NewSession {
        cwd: match cwd {
            Some(cwd) => cwd,
            None => current_dir()?,
        },
        agent,
        task,
    };

    Ok(ParsedCli::Request(request))
}

fn parse_focus_command(args: impl Iterator<Item = String>) -> Result<ParsedCli, CliError> {
    let mut id = None;
    let mut args = args.peekable();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(ParsedCli::Help(HelpTopic::Focus)),
            "--id" | "-i" => {
                id = Some(next_value(&mut args, "--id", HelpTopic::Focus)?);
            }
            _ if arg.starts_with('-') => {
                return Err(CliError::new(
                    format!("unknown option '{arg}'"),
                    HelpTopic::Focus,
                ));
            }
            _ if id.is_none() => id = Some(arg),
            _ => {
                return Err(CliError::new(
                    format!("unexpected argument '{arg}'"),
                    HelpTopic::Focus,
                ));
            }
        }
    }

    let id = id.ok_or_else(|| CliError::new("focus requires a session id", HelpTopic::Focus))?;

    Ok(ParsedCli::Request(ApiRequest::FocusSession { id }))
}

fn print_request(request: &ApiRequest) {
    println!(
        "{}",
        serde_json::to_string_pretty(request).expect("request serializes")
    );
}

fn help_text(topic: HelpTopic) -> String {
    let agents = "shell, codex, claude, opencode, gemini, aider";

    match topic {
        HelpTopic::General => format!(
            "\
lazyterm - preview the API request that would be sent

USAGE:
    lazyterm <command> [options]

COMMANDS:
    list               preview a ListSessions request
    status             preview a Status request
    new                preview a NewSession request for the shell agent
    run                preview a NewSession request for the codex agent
    focus <id>         preview a FocusSession request
    help [command]     show help for a command

OPTIONS:
    -h, --help         show help for the current command
    -C, --cwd <path>   set the working directory for new/run
    -a, --agent <kind> choose the agent for new/run ({agents})
    -t, --task <text>  set the task text for new/run

EXAMPLES:
    lazyterm list
    lazyterm run --cwd . --task \"fix the parser\"
    lazyterm new --agent shell --task \"inspect the repo\"
    lazyterm focus session-123
",
        ),
        HelpTopic::List => "\
lazyterm list

Preview a ListSessions request.
"
        .to_string(),
        HelpTopic::Status => "\
lazyterm status

Preview a Status request.
"
        .to_string(),
        HelpTopic::New => format!(
            "\
lazyterm new [options]

Preview a NewSession request with the shell agent by default.

OPTIONS:
    -h, --help         show help for this command
    -C, --cwd <path>   set the working directory
    -a, --agent <kind> choose the agent ({agents})
    -t, --task <text>  set the task text
"
        ),
        HelpTopic::Run => format!(
            "\
lazyterm run [options]

Preview a NewSession request with the codex agent by default.

OPTIONS:
    -h, --help         show help for this command
    -C, --cwd <path>   set the working directory
    -a, --agent <kind> choose the agent ({agents})
    -t, --task <text>  set the task text
"
        ),
        HelpTopic::Focus => "\
lazyterm focus <id>

Preview a FocusSession request.
"
        .to_string(),
    }
}

fn current_dir() -> Result<PathBuf, CliError> {
    std::env::current_dir().map_err(|err| {
        CliError::new(
            format!("failed to resolve the current directory: {err}"),
            HelpTopic::General,
        )
    })
}

fn next_value(
    args: &mut impl Iterator<Item = String>,
    flag: &str,
    help: HelpTopic,
) -> Result<String, CliError> {
    args.next()
        .ok_or_else(|| CliError::new(format!("{flag} requires a value"), help))
}

fn collect_task_text(
    args: impl Iterator<Item = String>,
    help: HelpTopic,
    label: &str,
) -> Result<String, CliError> {
    let task = args.collect::<Vec<_>>().join(" ");

    if task.is_empty() {
        Err(CliError::new(format!("{label} requires text"), help))
    } else {
        Ok(task)
    }
}

fn parse_agent_kind(value: &str, help: HelpTopic) -> Result<AgentKind, CliError> {
    match value.to_ascii_lowercase().as_str() {
        "shell" => Ok(AgentKind::Shell),
        "codex" => Ok(AgentKind::Codex),
        "claude" => Ok(AgentKind::Claude),
        "opencode" | "open-code" | "open_code" => Ok(AgentKind::OpenCode),
        "gemini" => Ok(AgentKind::Gemini),
        "aider" => Ok(AgentKind::Aider),
        _ => Err(CliError::new(
            format!(
                "unknown agent kind '{value}' (expected shell, codex, claude, opencode, gemini, or aider)"
            ),
            help,
        )),
    }
}

fn is_help_flag(value: &str) -> bool {
    matches!(value, "-h" | "--help")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<ParsedCli, CliError> {
        parse_cli(args.iter().map(|value| value.to_string()))
    }

    #[test]
    fn parses_run_with_task_text_and_cwd() {
        let parsed =
            parse(&["run", "--cwd", ".", "--task", "fix", "the", "parser"]).expect("parsed");

        assert_eq!(
            parsed,
            ParsedCli::Request(ApiRequest::NewSession {
                cwd: PathBuf::from("."),
                agent: AgentKind::Codex,
                task: Some("fix the parser".to_string()),
            })
        );
    }

    #[test]
    fn parses_new_with_explicit_agent_and_positional_task() {
        let parsed = parse(&[
            "new",
            "--cwd",
            ".",
            "--agent",
            "open-code",
            "inspect",
            "repo",
        ])
        .expect("parsed");

        assert_eq!(
            parsed,
            ParsedCli::Request(ApiRequest::NewSession {
                cwd: PathBuf::from("."),
                agent: AgentKind::OpenCode,
                task: Some("inspect repo".to_string()),
            })
        );
    }

    #[test]
    fn parses_focus_with_explicit_id() {
        let parsed = parse(&["focus", "--id", "session-123"]).expect("parsed");

        assert_eq!(
            parsed,
            ParsedCli::Request(ApiRequest::FocusSession {
                id: "session-123".to_string(),
            })
        );
    }

    #[test]
    fn rejects_focus_without_an_id() {
        let err = parse(&["focus"]).expect_err("error");

        assert_eq!(err.help, HelpTopic::Focus);
        assert!(err.message.contains("focus requires a session id"));
    }

    #[test]
    fn rejects_unknown_command() {
        let err = parse(&["bogus"]).expect_err("error");

        assert_eq!(err.help, HelpTopic::General);
        assert!(err.message.contains("unknown command"));
    }

    #[test]
    fn parses_help_for_command_topics() {
        assert_eq!(
            parse(&["help", "run"]).expect("parsed"),
            ParsedCli::Help(HelpTopic::Run)
        );
    }

    #[test]
    fn rejects_unknown_agent_kind() {
        let err = parse(&["run", "--agent", "not-real"]).expect_err("error");

        assert_eq!(err.help, HelpTopic::Run);
        assert!(err.message.contains("unknown agent kind"));
    }
}
