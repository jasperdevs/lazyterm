use std::fmt;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::ExitCode;

use lazyterm_api::{ApiRequest, ApiResponse, TerminalDensity, TileLayout};
use lazyterm_core::AgentKind;

const API_ADDR: &str = "127.0.0.1:47431";

fn main() -> ExitCode {
    match parse_cli(std::env::args().skip(1)) {
        Ok(ParsedCli::Help(topic)) => {
            println!("{}", help_text(topic));
            ExitCode::SUCCESS
        }
        Ok(ParsedCli::Request(request)) => match send_request(&request) {
            Ok(response) => {
                print_response(&response);
                match response {
                    ApiResponse::Error { .. } => ExitCode::from(1),
                    _ => ExitCode::SUCCESS,
                }
            }
            Err(error) => {
                eprintln!("error: failed to reach lazyterm app on {API_ADDR}: {error}");
                eprintln!("start lazyterm, then retry the command");
                ExitCode::from(1)
            }
        },
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
    CloseOthers,
    Attention,
    Layout,
    Density,
    Agents,
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
            "close-others" => {
                parse_no_arg_command(args, HelpTopic::CloseOthers, ApiRequest::CloseOtherSessions)
            }
            "attention" => {
                parse_no_arg_command(args, HelpTopic::Attention, ApiRequest::FocusAttention)
            }
            "layout" => parse_layout_command(args),
            "density" => parse_density_command(args),
            "agents" => parse_no_arg_command(args, HelpTopic::Agents, ApiRequest::AgentHealth),
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
                "close-others" => Ok(ParsedCli::Help(HelpTopic::CloseOthers)),
                "attention" => Ok(ParsedCli::Help(HelpTopic::Attention)),
                "layout" => Ok(ParsedCli::Help(HelpTopic::Layout)),
                "density" => Ok(ParsedCli::Help(HelpTopic::Density)),
                "agents" => Ok(ParsedCli::Help(HelpTopic::Agents)),
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

fn parse_layout_command(args: impl Iterator<Item = String>) -> Result<ParsedCli, CliError> {
    let mut layout = None;

    for arg in args {
        match arg.as_str() {
            "-h" | "--help" => return Ok(ParsedCli::Help(HelpTopic::Layout)),
            _ if arg.starts_with('-') => {
                return Err(CliError::new(
                    format!("unknown option '{arg}'"),
                    HelpTopic::Layout,
                ));
            }
            _ if layout.is_none() => layout = Some(parse_tile_layout(&arg)?),
            _ => {
                return Err(CliError::new(
                    format!("unexpected argument '{arg}'"),
                    HelpTopic::Layout,
                ));
            }
        }
    }

    let layout = layout.ok_or_else(|| {
        CliError::new("layout requires grid, columns, or rows", HelpTopic::Layout)
    })?;

    Ok(ParsedCli::Request(ApiRequest::SetLayout { layout }))
}

fn parse_density_command(args: impl Iterator<Item = String>) -> Result<ParsedCli, CliError> {
    let mut density = None;

    for arg in args {
        match arg.as_str() {
            "-h" | "--help" => return Ok(ParsedCli::Help(HelpTopic::Density)),
            _ if arg.starts_with('-') => {
                return Err(CliError::new(
                    format!("unknown option '{arg}'"),
                    HelpTopic::Density,
                ));
            }
            _ if density.is_none() => density = Some(parse_terminal_density(&arg)?),
            _ => {
                return Err(CliError::new(
                    format!("unexpected argument '{arg}'"),
                    HelpTopic::Density,
                ));
            }
        }
    }

    let density = density.ok_or_else(|| {
        CliError::new(
            "density requires compact, default, or roomy",
            HelpTopic::Density,
        )
    })?;

    Ok(ParsedCli::Request(ApiRequest::SetDensity { density }))
}

fn send_request(request: &ApiRequest) -> std::io::Result<ApiResponse> {
    let mut stream = TcpStream::connect(API_ADDR)?;
    serde_json::to_writer(&mut stream, request).map_err(std::io::Error::other)?;
    stream.write_all(b"\n")?;
    stream.flush()?;

    let mut response = String::new();
    BufReader::new(stream).read_line(&mut response)?;
    serde_json::from_str(response.trim_end()).map_err(std::io::Error::other)
}

fn print_response(response: &ApiResponse) {
    println!(
        "{}",
        serde_json::to_string_pretty(response).expect("response serializes")
    );
}

fn help_text(topic: HelpTopic) -> String {
    let agents = "shell, codex, claude, opencode, gemini, aider";

    match topic {
        HelpTopic::General => format!(
            "\
lazytermctl - control the running Lazyterm app

USAGE:
    lazytermctl <command> [options]

COMMANDS:
    list               list sessions in the running app
    status             show current app session status
    agents             show local agent command availability
    new                create a shell session
    run                create a codex session
    focus <id>         focus a session
    attention          focus the next pane that needs attention
    close-others       close every pane except the active one
    layout <mode>      set pane layout: grid, columns, rows
    density <mode>     set terminal density: compact, default, roomy
    help [command]     show help for a command

OPTIONS:
    -h, --help         show help for the current command
    -C, --cwd <path>   set the working directory for new/run
    -a, --agent <kind> choose the agent for new/run ({agents})
    -t, --task <text>  set the task text for new/run

EXAMPLES:
    lazytermctl list
    lazytermctl agents
    lazytermctl run --cwd . --task \"fix the parser\"
    lazytermctl new --agent shell --task \"inspect the repo\"
    lazytermctl focus session-123
    lazytermctl close-others
    lazytermctl layout columns
    lazytermctl density compact
",
        ),
        HelpTopic::List => "\
lazytermctl list

List sessions in the running app.
"
        .to_string(),
        HelpTopic::Status => "\
lazytermctl status

Show session status from the running app.
"
        .to_string(),
        HelpTopic::New => format!(
            "\
lazytermctl new [options]

Create a new session with the shell agent by default.

OPTIONS:
    -h, --help         show help for this command
    -C, --cwd <path>   set the working directory
    -a, --agent <kind> choose the agent ({agents})
    -t, --task <text>  set the task text
"
        ),
        HelpTopic::Run => format!(
            "\
lazytermctl run [options]

Create a new session with the codex agent by default.

OPTIONS:
    -h, --help         show help for this command
    -C, --cwd <path>   set the working directory
    -a, --agent <kind> choose the agent ({agents})
    -t, --task <text>  set the task text
"
        ),
        HelpTopic::Focus => "\
lazytermctl focus <id>

Focus a session in the running app.
"
        .to_string(),
        HelpTopic::CloseOthers => "\
lazytermctl close-others

Close every pane except the active pane.
"
        .to_string(),
        HelpTopic::Attention => "\
lazytermctl attention

Focus the next pane that needs input or failed.
"
        .to_string(),
        HelpTopic::Layout => "\
lazytermctl layout <grid|columns|rows>

Set the tiled pane layout.
"
        .to_string(),
        HelpTopic::Density => "\
lazytermctl density <compact|default|roomy>

Set terminal font and padding density.
"
        .to_string(),
        HelpTopic::Agents => "\
lazytermctl agents

Show whether shell, Codex, Claude, OpenCode, Gemini, and Aider commands are available.
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

fn parse_tile_layout(value: &str) -> Result<TileLayout, CliError> {
    match value.to_ascii_lowercase().as_str() {
        "grid" => Ok(TileLayout::Grid),
        "columns" | "column" | "cols" => Ok(TileLayout::Columns),
        "rows" | "row" => Ok(TileLayout::Rows),
        _ => Err(CliError::new(
            format!("unknown layout '{value}' (expected grid, columns, or rows)"),
            HelpTopic::Layout,
        )),
    }
}

fn parse_terminal_density(value: &str) -> Result<TerminalDensity, CliError> {
    match value.to_ascii_lowercase().as_str() {
        "compact" => Ok(TerminalDensity::Compact),
        "default" | "normal" => Ok(TerminalDensity::Default),
        "roomy" | "large" => Ok(TerminalDensity::Roomy),
        _ => Err(CliError::new(
            format!("unknown density '{value}' (expected compact, default, or roomy)"),
            HelpTopic::Density,
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
    fn parses_pane_control_commands() {
        assert_eq!(
            parse(&["close-others"]).expect("parsed"),
            ParsedCli::Request(ApiRequest::CloseOtherSessions)
        );
        assert_eq!(
            parse(&["attention"]).expect("parsed"),
            ParsedCli::Request(ApiRequest::FocusAttention)
        );
    }

    #[test]
    fn parses_layout_and_density_commands() {
        assert_eq!(
            parse(&["layout", "columns"]).expect("parsed"),
            ParsedCli::Request(ApiRequest::SetLayout {
                layout: TileLayout::Columns,
            })
        );
        assert_eq!(
            parse(&["density", "compact"]).expect("parsed"),
            ParsedCli::Request(ApiRequest::SetDensity {
                density: TerminalDensity::Compact,
            })
        );
        assert_eq!(
            parse(&["agents"]).expect("parsed"),
            ParsedCli::Request(ApiRequest::AgentHealth)
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
