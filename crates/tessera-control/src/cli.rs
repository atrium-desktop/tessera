//! Domain-oriented command structure parsed directly from argv.

use std::fmt;
use std::io::Write;
use std::path::PathBuf;
use std::str::FromStr;

use tessera_primitives::Rect;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseErrorKind {
    DisplayHelp,
    DisplayVersion,
    InvalidValue,
    MissingSubcommand,
    Usage,
}

#[derive(Debug, Clone)]
pub struct ParseError {
    pub kind: ParseErrorKind,
    pub message: String,
}

impl ParseError {
    pub fn new(kind: ParseErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn use_stderr(&self) -> bool {
        !matches!(
            self.kind,
            ParseErrorKind::DisplayHelp | ParseErrorKind::DisplayVersion
        )
    }

    pub fn print(&self) -> std::io::Result<()> {
        if self.use_stderr() {
            let mut err = std::io::stderr().lock();
            writeln!(err, "{}", self.message)
        } else {
            let mut out = std::io::stdout().lock();
            writeln!(out, "{}", self.message)
        }
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ParseError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shell {
    Bash,
    Elvish,
    Fish,
    PowerShell,
    Zsh,
}

impl FromStr for Shell {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "bash" => Ok(Self::Bash),
            "elvish" => Ok(Self::Elvish),
            "fish" => Ok(Self::Fish),
            "powershell" => Ok(Self::PowerShell),
            "zsh" => Ok(Self::Zsh),
            _ => Err(format!(
                "invalid shell '{s}'; expected bash, elvish, fish, powershell, or zsh"
            )),
        }
    }
}

impl Shell {
    pub fn generate(self, out: &mut dyn Write) {
        let cmds = "run calc query display window workspace notification journal interaction-domain permissions config system overview events completions quit";
        match self {
            Self::Bash => {
                let _ = writeln!(
                    out,
                    "_tessera() {{\n  local cur=\"${{COMP_WORDS[COMP_CWORD]}}\"\n  if [[ $COMP_CWORD -eq 1 ]]; then\n    COMPREPLY=( $(compgen -W \"{cmds}\" -- \"$cur\") )\n  fi\n}}\ncomplete -F _tessera tessera"
                );
            }
            Self::Zsh => {
                let _ = writeln!(out, "#compdef tessera\n_arguments '1:command:({cmds})'");
            }
            Self::Fish => {
                for c in cmds.split_whitespace() {
                    let _ = writeln!(
                        out,
                        "complete -c tessera -n '__fish_use_subcommand' -a '{c}'"
                    );
                }
            }
            Self::Elvish => {
                let _ = writeln!(
                    out,
                    "set edit:completion:arg-completer[tessera] = {{|@args| put {cmds} | each {{|c| echo $c }} }}"
                );
            }
            Self::PowerShell => {
                let _ = writeln!(
                    out,
                    "Register-ArgumentCompleter -Native -CommandName tessera -ScriptBlock {{ param($wordToComplete) '{cmds}'.Split(' ') | Where-Object {{ $_ -like \"$wordToComplete*\" }} }}"
                );
            }
        }
    }
}

/// Tessera compositor and session management.
#[derive(Debug)]
pub struct Cli {
    pub json: bool,
    pub socket: Option<PathBuf>,
    pub command: Option<Command>,
}

impl Cli {
    pub fn runs_compositor(&self) -> bool {
        self.command
            .as_ref()
            .is_none_or(|command| matches!(command, Command::Run))
    }

    pub fn try_parse() -> Result<Self, ParseError> {
        Self::try_parse_from(std::env::args())
    }

    pub fn try_parse_from<I, T>(itr: I) -> Result<Self, ParseError>
    where
        I: IntoIterator<Item = T>,
        T: Into<String>,
    {
        let raw: Vec<String> = itr.into_iter().map(Into::into).collect();
        let mut rest: Vec<String> = raw.into_iter().skip(1).collect();

        let mut json = false;
        let mut socket: Option<PathBuf> = None;
        let mut filtered: Vec<String> = Vec::with_capacity(rest.len());

        let mut i = 0;
        while i < rest.len() {
            let arg = &rest[i];
            if arg == "--" {
                filtered.extend(rest.drain(i..));
                break;
            }
            match arg.as_str() {
                "-j" | "--json" => {
                    json = true;
                    i += 1;
                }
                "--socket" => {
                    i += 1;
                    let val = rest.get(i).ok_or_else(|| {
                        ParseError::new(ParseErrorKind::Usage, "error: --socket requires a value")
                    })?;
                    socket = Some(PathBuf::from(val));
                    i += 1;
                }
                s if s.starts_with("--socket=") => {
                    socket = Some(PathBuf::from(&s["--socket=".len()..]));
                    i += 1;
                }
                "-V" | "--version" if filtered.is_empty() => {
                    return Err(ParseError::new(
                        ParseErrorKind::DisplayVersion,
                        format!("tessera {}", env!("CARGO_PKG_VERSION")),
                    ));
                }
                "-h" | "--help" if filtered.is_empty() => {
                    return Err(ParseError::new(ParseErrorKind::DisplayHelp, main_help()));
                }
                _ => {
                    filtered.push(arg.clone());
                    i += 1;
                }
            }
        }

        if filtered.is_empty() {
            return Ok(Self {
                json,
                socket,
                command: None,
            });
        }

        let command = parse_command(&filtered)?;
        Ok(Self {
            json,
            socket,
            command: Some(command),
        })
    }
}

fn main_help() -> String {
    "Tessera compositor and session management\n\n\
     Usage: tessera [OPTIONS] [COMMAND]\n\n\
     Commands:\n  \
       run                  Start the compositor explicitly\n  \
       calc                 Evaluate arithmetic without connecting to a session\n  \
       query                Resolve a search query without connecting to a session\n  \
       display              Inspect displays and capture the focused display\n  \
       window               Inspect and control windows\n  \
       workspace            Inspect and control workspaces\n  \
       notification         Inspect and post notifications\n  \
       journal              Inspect or follow the unified observability event bus\n  \
       interaction-domain   Manage Agent Workspaces and Interaction Domain authority\n  \
       permissions          Manage agent permissions and grants\n  \
       config               Validate or explicitly migrate the local Tessera configuration\n  \
       system               Inspect and control immediate live-system state\n  \
       overview             Toggle the window/workspace overview\n  \
       events               Stream coarse compositor state-change events\n  \
       completions          Print shell completions for the given shell to stdout\n  \
       quit                 Request compositor shutdown\n\n\
     Options:\n  \
       -j, --json           Emit query results and receipts as JSON\n  \
       --socket <SOCKET>    Explicit session IPC socket\n  \
       -h, --help           Print help\n  \
       -V, --version        Print version"
        .to_string()
}

fn usage_err(msg: impl Into<String>) -> ParseError {
    ParseError::new(
        ParseErrorKind::Usage,
        format!(
            "error: {}\n\nUsage: tessera [OPTIONS] [COMMAND]",
            msg.into()
        ),
    )
}

fn check_help(args: &[String], help: &str) -> Result<(), ParseError> {
    if args.iter().any(|a| a == "-h" || a == "--help") {
        return Err(ParseError::new(ParseErrorKind::DisplayHelp, help));
    }
    Ok(())
}

fn parse_val<T: FromStr>(raw: &str, name: &str) -> Result<T, ParseError>
where
    T::Err: fmt::Display,
{
    raw.parse::<T>().map_err(|e| {
        ParseError::new(
            ParseErrorKind::InvalidValue,
            format!("invalid {name} '{raw}': {e}"),
        )
    })
}

fn parse_range_i8(raw: &str, min: i8, max: i8, name: &str) -> Result<i8, ParseError> {
    let v: i8 = parse_val(raw, name)?;
    if !(min..=max).contains(&v) {
        return Err(ParseError::new(
            ParseErrorKind::InvalidValue,
            format!("{name} must be in {min}..={max}, got {v}"),
        ));
    }
    Ok(v)
}

fn parse_range_u8(raw: &str, min: u8, max: u8, name: &str) -> Result<u8, ParseError> {
    let v: u8 = parse_val(raw, name)?;
    if !(min..=max).contains(&v) {
        return Err(ParseError::new(
            ParseErrorKind::InvalidValue,
            format!("{name} must be in {min}..={max}, got {v}"),
        ));
    }
    Ok(v)
}

fn next_opt_val<'a>(args: &'a [String], i: &mut usize, flag: &str) -> Result<&'a str, ParseError> {
    *i += 1;
    args.get(*i)
        .map(String::as_str)
        .ok_or_else(|| usage_err(format!("{flag} requires a value")))
}

fn parse_command(args: &[String]) -> Result<Command, ParseError> {
    let (cmd, rest) = (&args[0], &args[1..]);
    if cmd == "help" {
        return Err(ParseError::new(ParseErrorKind::DisplayHelp, main_help()));
    }
    match cmd.as_str() {
        "run" => {
            ensure_no_extra(rest, "run")?;
            Ok(Command::Run)
        }
        "calc" => {
            check_help(rest, "Usage: tessera calc <EXPRESSION>")?;
            if rest.len() != 1 {
                return Err(usage_err("Usage: tessera calc <EXPRESSION>"));
            }
            Ok(Command::Calc {
                expression: rest[0].clone(),
            })
        }
        "query" => {
            check_help(rest, "Usage: tessera query <QUERY>")?;
            if rest.len() != 1 {
                return Err(usage_err("Usage: tessera query <QUERY>"));
            }
            Ok(Command::Query {
                query: rest[0].clone(),
            })
        }
        "display" => Ok(Command::Display {
            command: parse_display(rest)?,
        }),
        "window" => Ok(Command::Window {
            command: parse_window(rest)?,
        }),
        "workspace" => Ok(Command::Workspace {
            command: parse_workspace(rest)?,
        }),
        "notification" => Ok(Command::Notification {
            command: parse_notification(rest)?,
        }),
        "journal" => Ok(Command::Journal {
            command: parse_journal(rest)?,
        }),
        "interaction-domain" => Ok(Command::InteractionDomain {
            command: parse_interaction_domain(rest)?,
        }),
        "permissions" => Ok(Command::Permissions {
            command: parse_permissions(rest)?,
        }),
        "config" => Ok(Command::Config {
            command: parse_config(rest)?,
        }),
        "system" => Ok(Command::System {
            command: parse_system(rest)?,
        }),
        "overview" => {
            ensure_no_extra(rest, "overview")?;
            Ok(Command::Overview)
        }
        "events" => {
            ensure_no_extra(rest, "events")?;
            Ok(Command::Events)
        }
        "completions" => {
            check_help(
                rest,
                "Usage: tessera completions <bash|elvish|fish|powershell|zsh>",
            )?;
            if rest.len() != 1 {
                return Err(usage_err("Usage: tessera completions <SHELL>"));
            }
            Ok(Command::Completions {
                shell: parse_val(&rest[0], "shell")?,
            })
        }
        "quit" => {
            ensure_no_extra(rest, "quit")?;
            Ok(Command::Quit)
        }
        other => Err(usage_err(format!("unrecognized subcommand '{other}'"))),
    }
}

fn ensure_no_extra(args: &[String], name: &str) -> Result<(), ParseError> {
    check_help(args, &format!("Usage: tessera {name}"))?;
    if let Some(first) = args.first() {
        return Err(usage_err(format!(
            "unexpected argument '{first}' for {name}"
        )));
    }
    Ok(())
}

fn parse_config(args: &[String]) -> Result<Option<ConfigCmd>, ParseError> {
    check_help(args, "Usage: tessera config [validate|migrate] [PATH]")?;
    let Some((sub, rest)) = args.split_first() else {
        return Ok(None);
    };
    let path = match rest.len() {
        0 => None,
        1 => Some(PathBuf::from(&rest[0])),
        _ => return Err(usage_err("unexpected extra arguments for config")),
    };
    match sub.as_str() {
        "validate" => Ok(Some(ConfigCmd::Validate { path })),
        "migrate" => Ok(Some(ConfigCmd::Migrate { path })),
        other => Err(usage_err(format!("unknown config subcommand '{other}'"))),
    }
}

fn parse_display(args: &[String]) -> Result<Option<DisplayCmd>, ParseError> {
    check_help(
        args,
        "Usage: tessera display [list|capture|virtual|confirm|topology|role]",
    )?;
    let Some((sub, rest)) = args.split_first() else {
        return Ok(None);
    };
    match sub.as_str() {
        "list" => {
            ensure_no_extra(rest, "display list")?;
            Ok(Some(DisplayCmd::List))
        }
        "confirm" => {
            ensure_no_extra(rest, "display confirm")?;
            Ok(Some(DisplayCmd::Confirm))
        }
        "capture" => {
            let mut path = None;
            let mut region = None;
            let mut i = 0;
            while i < rest.len() {
                match rest[i].as_str() {
                    "--region" => {
                        let v = next_opt_val(rest, &mut i, "--region")?;
                        region = Some(parse_val::<Region>(v, "region")?);
                    }
                    s if s.starts_with("--region=") => {
                        region = Some(parse_val::<Region>(&s["--region=".len()..], "region")?);
                    }
                    s if !s.starts_with('-') && path.is_none() => {
                        path = Some(s.to_string());
                    }
                    other => return Err(usage_err(format!("unexpected argument '{other}'"))),
                }
                i += 1;
            }
            Ok(Some(DisplayCmd::Capture { path, region }))
        }
        "virtual" => Ok(Some(DisplayCmd::Virtual {
            command: parse_virtual_display(rest)?,
        })),
        "topology" => {
            if rest.len() != 1 {
                return Err(usage_err(
                    "Usage: tessera display topology <auto|extend|mirror>",
                ));
            }
            Ok(Some(DisplayCmd::Topology {
                topology: parse_val(&rest[0], "topology")?,
            }))
        }
        "role" => {
            let mut source = None;
            let mut pos = Vec::new();
            let mut i = 0;
            while i < rest.len() {
                match rest[i].as_str() {
                    "--source" => {
                        source = Some(next_opt_val(rest, &mut i, "--source")?.to_string());
                    }
                    s if s.starts_with("--source=") => {
                        source = Some(s["--source=".len()..].to_string());
                    }
                    s => pos.push(s),
                }
                i += 1;
            }
            if pos.len() != 2 {
                return Err(usage_err(
                    "Usage: tessera display role <CONNECTOR> <automatic|extend|mirror> [--source SRC]",
                ));
            }
            Ok(Some(DisplayCmd::Role {
                connector: pos[0].to_string(),
                role: parse_val(pos[1], "role")?,
                source,
            }))
        }
        other => Err(usage_err(format!("unknown display subcommand '{other}'"))),
    }
}

fn parse_virtual_display(args: &[String]) -> Result<VirtualDisplayCmd, ParseError> {
    check_help(
        args,
        "Usage: tessera display virtual <create|resize|destroy>",
    )?;
    let (sub, rest) = args
        .split_first()
        .ok_or_else(|| usage_err("Usage: tessera display virtual <create|resize|destroy>"))?;
    match sub.as_str() {
        "create" => {
            let mut connector = None;
            let mut mode = "1920x1080@60".to_string();
            let mut scale = 1.0f32;
            let mut x = None;
            let mut y = None;
            let mut i = 0;
            while i < rest.len() {
                match rest[i].as_str() {
                    "--connector" => {
                        connector = Some(next_opt_val(rest, &mut i, "--connector")?.to_string())
                    }
                    "--mode" => mode = next_opt_val(rest, &mut i, "--mode")?.to_string(),
                    "--scale" => {
                        scale = parse_val(next_opt_val(rest, &mut i, "--scale")?, "scale")?
                    }
                    "--x" => x = Some(parse_val(next_opt_val(rest, &mut i, "--x")?, "x")?),
                    "--y" => y = Some(parse_val(next_opt_val(rest, &mut i, "--y")?, "y")?),
                    other => return Err(usage_err(format!("unexpected argument '{other}'"))),
                }
                i += 1;
            }
            Ok(VirtualDisplayCmd::Create {
                connector,
                mode,
                scale,
                x,
                y,
            })
        }
        "resize" => {
            let mut connector = None;
            let mut mode = None;
            let mut scale = None;
            let mut x = None;
            let mut y = None;
            let mut i = 0;
            while i < rest.len() {
                match rest[i].as_str() {
                    "--mode" => mode = Some(next_opt_val(rest, &mut i, "--mode")?.to_string()),
                    "--scale" => {
                        scale = Some(parse_val(next_opt_val(rest, &mut i, "--scale")?, "scale")?)
                    }
                    "--x" => x = Some(parse_val(next_opt_val(rest, &mut i, "--x")?, "x")?),
                    "--y" => y = Some(parse_val(next_opt_val(rest, &mut i, "--y")?, "y")?),
                    s if connector.is_none() => connector = Some(s.to_string()),
                    other => return Err(usage_err(format!("unexpected argument '{other}'"))),
                }
                i += 1;
            }
            Ok(VirtualDisplayCmd::Resize {
                connector: connector.ok_or_else(|| usage_err("resize requires <CONNECTOR>"))?,
                mode: mode.ok_or_else(|| usage_err("resize requires --mode"))?,
                scale,
                x,
                y,
            })
        }
        "destroy" => {
            if rest.len() != 1 {
                return Err(usage_err(
                    "Usage: tessera display virtual destroy <CONNECTOR>",
                ));
            }
            Ok(VirtualDisplayCmd::Destroy {
                connector: rest[0].clone(),
            })
        }
        other => Err(usage_err(format!(
            "unknown virtual display subcommand '{other}'"
        ))),
    }
}

fn parse_window(args: &[String]) -> Result<Option<WindowCmd>, ParseError> {
    check_help(
        args,
        "Usage: tessera window [list|focus|minimize|always-on-top|fullscreen|close|geometry]",
    )?;
    let Some((sub, rest)) = args.split_first() else {
        return Ok(None);
    };
    match sub.as_str() {
        "list" => {
            ensure_no_extra(rest, "window list")?;
            Ok(Some(WindowCmd::List))
        }
        "focus" => {
            if rest.len() != 1 {
                return Err(usage_err("Usage: tessera window focus <ID>"));
            }
            Ok(Some(WindowCmd::Focus {
                id: parse_val(&rest[0], "id")?,
            }))
        }
        "minimize" => {
            if rest.len() != 1 {
                return Err(usage_err("Usage: tessera window minimize <ID>"));
            }
            Ok(Some(WindowCmd::Minimize {
                id: parse_val(&rest[0], "id")?,
            }))
        }
        "always-on-top" => {
            if rest.len() != 2 {
                return Err(usage_err(
                    "Usage: tessera window always-on-top <ID> <on|off>",
                ));
            }
            Ok(Some(WindowCmd::AlwaysOnTop {
                id: parse_val(&rest[0], "id")?,
                state: parse_val(&rest[1], "state")?,
            }))
        }
        "fullscreen" => {
            if rest.len() != 2 {
                return Err(usage_err("Usage: tessera window fullscreen <ID> <on|off>"));
            }
            Ok(Some(WindowCmd::Fullscreen {
                id: parse_val(&rest[0], "id")?,
                state: parse_val(&rest[1], "state")?,
            }))
        }
        "close" => {
            if rest.len() != 1 {
                return Err(usage_err("Usage: tessera window close <ID>"));
            }
            Ok(Some(WindowCmd::Close {
                id: parse_val(&rest[0], "id")?,
            }))
        }
        "geometry" => {
            if rest.len() != 5 {
                return Err(usage_err(
                    "Usage: tessera window geometry <ID> <X> <Y> <WIDTH> <HEIGHT>",
                ));
            }
            Ok(Some(WindowCmd::Geometry {
                id: parse_val(&rest[0], "id")?,
                x: parse_val(&rest[1], "x")?,
                y: parse_val(&rest[2], "y")?,
                width: parse_val(&rest[3], "width")?,
                height: parse_val(&rest[4], "height")?,
            }))
        }
        other => Err(usage_err(format!("unknown window subcommand '{other}'"))),
    }
}

fn parse_workspace(args: &[String]) -> Result<Option<WorkspaceCmd>, ParseError> {
    check_help(args, "Usage: tessera workspace [list|switch|move-window]")?;
    let Some((sub, rest)) = args.split_first() else {
        return Ok(None);
    };
    match sub.as_str() {
        "list" => {
            ensure_no_extra(rest, "workspace list")?;
            Ok(Some(WorkspaceCmd::List))
        }
        "switch" => {
            if rest.len() != 1 {
                return Err(usage_err("Usage: tessera workspace switch <TARGET>"));
            }
            Ok(Some(WorkspaceCmd::Switch {
                target: parse_val(&rest[0], "target")?,
            }))
        }
        "move-window" => {
            if rest.len() != 2 {
                return Err(usage_err(
                    "Usage: tessera workspace move-window <WINDOW> <WORKSPACE>",
                ));
            }
            Ok(Some(WorkspaceCmd::MoveWindow {
                window: parse_val(&rest[0], "window")?,
                workspace: parse_val(&rest[1], "workspace")?,
            }))
        }
        other => Err(usage_err(format!("unknown workspace subcommand '{other}'"))),
    }
}

fn parse_notification(args: &[String]) -> Result<Option<NotificationCmd>, ParseError> {
    check_help(args, "Usage: tessera notification [list|send|dismiss]")?;
    let Some((sub, rest)) = args.split_first() else {
        return Ok(None);
    };
    match sub.as_str() {
        "list" => {
            ensure_no_extra(rest, "notification list")?;
            Ok(Some(NotificationCmd::List))
        }
        "send" => {
            if rest.is_empty() || rest.len() > 2 {
                return Err(usage_err(
                    "Usage: tessera notification send <SUMMARY> [BODY]",
                ));
            }
            Ok(Some(NotificationCmd::Send {
                summary: rest[0].clone(),
                body: rest.get(1).cloned(),
            }))
        }
        "dismiss" => {
            if rest.len() != 1 {
                return Err(usage_err("Usage: tessera notification dismiss <ID>"));
            }
            Ok(Some(NotificationCmd::Dismiss {
                id: parse_val(&rest[0], "id")?,
            }))
        }
        other => Err(usage_err(format!(
            "unknown notification subcommand '{other}'"
        ))),
    }
}

fn parse_event_topic(raw: &str) -> Result<tessera_protocol::EventTopic, ParseError> {
    match raw.to_ascii_lowercase().as_str() {
        "window" => Ok(tessera_protocol::EventTopic::Window),
        "workspace" => Ok(tessera_protocol::EventTopic::Workspace),
        "interaction-domain" | "interaction_domain" => {
            Ok(tessera_protocol::EventTopic::InteractionDomain)
        }
        "settings" => Ok(tessera_protocol::EventTopic::Settings),
        "authority" => Ok(tessera_protocol::EventTopic::Authority),
        "system" => Ok(tessera_protocol::EventTopic::System),
        "mcp" => Ok(tessera_protocol::EventTopic::Mcp),
        "custom" => Ok(tessera_protocol::EventTopic::Custom),
        other => Err(ParseError::new(
            ParseErrorKind::InvalidValue,
            format!(
                "unknown topic '{other}'; expected one of: window, workspace, interaction-domain, settings, authority, system, mcp, custom"
            ),
        )),
    }
}

fn parse_journal(args: &[String]) -> Result<Option<JournalCmd>, ParseError> {
    check_help(
        args,
        "Usage: tessera journal [list [--since <SEQ>] [--topic <TOPIC>]... | follow [--topic <TOPIC>]...]",
    )?;
    let Some((sub, rest)) = args.split_first() else {
        return Ok(None);
    };
    match sub.as_str() {
        "list" => {
            let mut since = None;
            let mut topics = Vec::new();
            let mut i = 0;
            while i < rest.len() {
                match rest[i].as_str() {
                    "--since" => {
                        since = Some(parse_val(next_opt_val(rest, &mut i, "--since")?, "since")?)
                    }
                    s if s.starts_with("--since=") => {
                        since = Some(parse_val(&s["--since=".len()..], "since")?);
                    }
                    "--topic" => {
                        topics.push(parse_event_topic(next_opt_val(rest, &mut i, "--topic")?)?);
                    }
                    s if s.starts_with("--topic=") => {
                        topics.push(parse_event_topic(&s["--topic=".len()..])?);
                    }
                    other => return Err(usage_err(format!("unexpected argument '{other}'"))),
                }
                i += 1;
            }
            Ok(Some(JournalCmd::List { since, topics }))
        }
        "follow" => {
            let mut topics = Vec::new();
            let mut i = 0;
            while i < rest.len() {
                match rest[i].as_str() {
                    "--topic" => {
                        topics.push(parse_event_topic(next_opt_val(rest, &mut i, "--topic")?)?);
                    }
                    s if s.starts_with("--topic=") => {
                        topics.push(parse_event_topic(&s["--topic=".len()..])?);
                    }
                    other => return Err(usage_err(format!("unexpected argument '{other}'"))),
                }
                i += 1;
            }
            Ok(Some(JournalCmd::Follow { topics }))
        }
        other => Err(usage_err(format!("unknown journal subcommand '{other}'"))),
    }
}

fn parse_system(args: &[String]) -> Result<Option<SystemCmd>, ParseError> {
    check_help(
        args,
        "Usage: tessera system [status|mute|step-volume|volume|audio-output|mic-mute|mic-volume|audio-input|brightness|wifi|bluetooth|do-not-disturb|power-mode|suspend|reboot|power-off]",
    )?;
    let Some((sub, rest)) = args.split_first() else {
        return Ok(None);
    };
    match sub.as_str() {
        "status" => {
            ensure_no_extra(rest, "system status")?;
            Ok(Some(SystemCmd::Status))
        }
        "mute" => {
            ensure_no_extra(rest, "system mute")?;
            Ok(Some(SystemCmd::Mute))
        }
        "step-volume" => {
            if rest.len() != 1 {
                return Err(usage_err("Usage: tessera system step-volume <DELTA>"));
            }
            Ok(Some(SystemCmd::StepVolume {
                delta: parse_range_i8(&rest[0], -100, 100, "delta")?,
            }))
        }
        "volume" => {
            if rest.len() != 1 {
                return Err(usage_err("Usage: tessera system volume <LEVEL>"));
            }
            Ok(Some(SystemCmd::Volume {
                level: parse_range_u8(&rest[0], 0, 100, "level")?,
            }))
        }
        "audio-output" => {
            if rest.len() != 1 {
                return Err(usage_err("Usage: tessera system audio-output <NAME>"));
            }
            Ok(Some(SystemCmd::AudioOutput {
                name: rest[0].clone(),
            }))
        }
        "mic-mute" => {
            ensure_no_extra(rest, "system mic-mute")?;
            Ok(Some(SystemCmd::MicMute))
        }
        "mic-volume" => {
            if rest.len() != 1 {
                return Err(usage_err("Usage: tessera system mic-volume <LEVEL>"));
            }
            Ok(Some(SystemCmd::MicVolume {
                level: parse_range_u8(&rest[0], 0, 100, "level")?,
            }))
        }
        "audio-input" => {
            if rest.len() != 1 {
                return Err(usage_err("Usage: tessera system audio-input <NAME>"));
            }
            Ok(Some(SystemCmd::AudioInput {
                name: rest[0].clone(),
            }))
        }
        "brightness" => {
            if rest.len() != 1 {
                return Err(usage_err("Usage: tessera system brightness <LEVEL>"));
            }
            Ok(Some(SystemCmd::Brightness {
                level: parse_range_u8(&rest[0], 1, 100, "level")?,
            }))
        }
        "wifi" => {
            if rest.len() != 1 {
                return Err(usage_err("Usage: tessera system wifi <on|off>"));
            }
            Ok(Some(SystemCmd::Wifi {
                state: parse_val(&rest[0], "state")?,
            }))
        }
        "bluetooth" => {
            if rest.len() != 1 {
                return Err(usage_err("Usage: tessera system bluetooth <on|off>"));
            }
            Ok(Some(SystemCmd::Bluetooth {
                state: parse_val(&rest[0], "state")?,
            }))
        }
        "do-not-disturb" => {
            if rest.len() != 1 {
                return Err(usage_err("Usage: tessera system do-not-disturb <on|off>"));
            }
            Ok(Some(SystemCmd::DoNotDisturb {
                state: parse_val(&rest[0], "state")?,
            }))
        }
        "power-mode" => {
            if rest.len() != 1 {
                return Err(usage_err(
                    "Usage: tessera system power-mode <balanced|awake|secure>",
                ));
            }
            Ok(Some(SystemCmd::PowerMode {
                mode: parse_val(&rest[0], "mode")?,
            }))
        }
        "suspend" => {
            ensure_no_extra(rest, "system suspend")?;
            Ok(Some(SystemCmd::Suspend))
        }
        "reboot" => {
            ensure_no_extra(rest, "system reboot")?;
            Ok(Some(SystemCmd::Reboot))
        }
        "power-off" => {
            ensure_no_extra(rest, "system power-off")?;
            Ok(Some(SystemCmd::PowerOff))
        }
        other => Err(usage_err(format!("unknown system subcommand '{other}'"))),
    }
}

fn parse_interaction_domain(args: &[String]) -> Result<Option<InteractionDomainCmd>, ParseError> {
    check_help(
        args,
        "Usage: tessera interaction-domain [list|create|pause|resume|transfer|launch|capture|revoke]",
    )?;
    let Some((sub, rest)) = args.split_first() else {
        return Ok(None);
    };
    match sub.as_str() {
        "list" => {
            ensure_no_extra(rest, "interaction-domain list")?;
            Ok(Some(InteractionDomainCmd::List))
        }
        "create" => {
            if rest.len() > 1 {
                return Err(usage_err(
                    "Usage: tessera interaction-domain create [LABEL]",
                ));
            }
            Ok(Some(InteractionDomainCmd::Create {
                label: rest
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "Agent Workspace".into()),
            }))
        }
        "pause" => {
            if rest.len() != 1 {
                return Err(usage_err("Usage: tessera interaction-domain pause <ID>"));
            }
            Ok(Some(InteractionDomainCmd::Pause {
                interaction_domain: parse_val(&rest[0], "interaction_domain")?,
            }))
        }
        "resume" => {
            if rest.len() != 1 {
                return Err(usage_err("Usage: tessera interaction-domain resume <ID>"));
            }
            Ok(Some(InteractionDomainCmd::Resume {
                interaction_domain: parse_val(&rest[0], "interaction_domain")?,
            }))
        }
        "transfer" => {
            let mut no_mirror = false;
            let mut pos = Vec::new();
            for a in rest {
                if a == "--no-mirror" {
                    no_mirror = true;
                } else {
                    pos.push(a);
                }
            }
            if pos.len() != 2 {
                return Err(usage_err(
                    "Usage: tessera interaction-domain transfer <WINDOW> <ID> [--no-mirror]",
                ));
            }
            Ok(Some(InteractionDomainCmd::Transfer {
                window: parse_val(pos[0], "window")?,
                interaction_domain: parse_val(pos[1], "interaction_domain")?,
                no_mirror,
            }))
        }
        "launch" => {
            if rest.len() != 2 {
                return Err(usage_err(
                    "Usage: tessera interaction-domain launch <ID> <DESKTOP_ID>",
                ));
            }
            Ok(Some(InteractionDomainCmd::Launch {
                interaction_domain: parse_val(&rest[0], "interaction_domain")?,
                desktop_id: rest[1].clone(),
            }))
        }
        "capture" => {
            let mut region = None;
            let mut pos = Vec::new();
            let mut i = 0;
            while i < rest.len() {
                match rest[i].as_str() {
                    "--region" => {
                        let v = next_opt_val(rest, &mut i, "--region")?;
                        region = Some(parse_val::<Region>(v, "region")?);
                    }
                    s if s.starts_with("--region=") => {
                        region = Some(parse_val::<Region>(&s["--region=".len()..], "region")?);
                    }
                    s => pos.push(s),
                }
                i += 1;
            }
            if pos.is_empty() || pos.len() > 2 {
                return Err(usage_err(
                    "Usage: tessera interaction-domain capture <ID> [PATH] [--region X,Y,W,H]",
                ));
            }
            Ok(Some(InteractionDomainCmd::Capture {
                interaction_domain: parse_val(pos[0], "interaction_domain")?,
                path: pos.get(1).map(|s| (*s).to_string()),
                region,
            }))
        }
        "revoke" => {
            if rest.is_empty() || rest.len() > 2 {
                return Err(usage_err(
                    "Usage: tessera interaction-domain revoke <ID> [FALLBACK]",
                ));
            }
            Ok(Some(InteractionDomainCmd::Revoke {
                interaction_domain: parse_val(&rest[0], "interaction_domain")?,
                fallback: rest
                    .get(1)
                    .map(|s| parse_val(s, "fallback"))
                    .transpose()?
                    .unwrap_or(1),
            }))
        }
        other => Err(usage_err(format!(
            "unknown interaction-domain subcommand '{other}'"
        ))),
    }
}

fn parse_op_list(raw: &str) -> Result<Vec<tessera_protocol::ActorCapability>, ParseError> {
    raw.split(',')
        .filter(|s| !s.is_empty())
        .map(|s| op_class(s).map_err(|e| ParseError::new(ParseErrorKind::InvalidValue, e)))
        .collect()
}

fn parse_permissions(args: &[String]) -> Result<Option<PermissionsCmd>, ParseError> {
    check_help(
        args,
        "Usage: tessera permissions [list|revoke|forget|rename|set-ceiling|register]",
    )?;
    let Some((sub, rest)) = args.split_first() else {
        return Ok(None);
    };
    match sub.as_str() {
        "list" => {
            ensure_no_extra(rest, "permissions list")?;
            Ok(Some(PermissionsCmd::List))
        }
        "revoke" => {
            if rest.len() != 2 {
                return Err(usage_err(
                    "Usage: tessera permissions revoke <PRINCIPAL> <OP>",
                ));
            }
            Ok(Some(PermissionsCmd::Revoke {
                principal: rest[0].clone(),
                op: op_class(&rest[1])
                    .map_err(|e| ParseError::new(ParseErrorKind::InvalidValue, e))?,
            }))
        }
        "forget" => {
            if rest.len() != 1 {
                return Err(usage_err("Usage: tessera permissions forget <PRINCIPAL>"));
            }
            Ok(Some(PermissionsCmd::Forget {
                principal: rest[0].clone(),
            }))
        }
        "rename" => {
            if rest.is_empty() || rest.len() > 2 {
                return Err(usage_err(
                    "Usage: tessera permissions rename <PRINCIPAL> [LABEL]",
                ));
            }
            Ok(Some(PermissionsCmd::Rename {
                principal: rest[0].clone(),
                label: rest.get(1).cloned(),
            }))
        }
        "set-ceiling" => {
            let mut pregrant = Vec::new();
            let mut gated = Vec::new();
            let mut pos = Vec::new();
            let mut i = 0;
            while i < rest.len() {
                match rest[i].as_str() {
                    "--pregrant" => {
                        pregrant.extend(parse_op_list(next_opt_val(rest, &mut i, "--pregrant")?)?);
                    }
                    s if s.starts_with("--pregrant=") => {
                        pregrant.extend(parse_op_list(&s["--pregrant=".len()..])?);
                    }
                    "--gated" => {
                        gated.extend(parse_op_list(next_opt_val(rest, &mut i, "--gated")?)?);
                    }
                    s if s.starts_with("--gated=") => {
                        gated.extend(parse_op_list(&s["--gated=".len()..])?);
                    }
                    s => pos.push(s),
                }
                i += 1;
            }
            if pos.len() != 1 {
                return Err(usage_err(
                    "Usage: tessera permissions set-ceiling <PRINCIPAL> [--pregrant ...] [--gated ...]",
                ));
            }
            Ok(Some(PermissionsCmd::SetCeiling {
                principal: pos[0].to_string(),
                pregrant,
                gated,
            }))
        }
        "register" => {
            let mut pregrant = Vec::new();
            let mut gated = Vec::new();
            let mut label = None;
            let mut i = 0;
            while i < rest.len() {
                match rest[i].as_str() {
                    "--pregrant" => {
                        pregrant.extend(parse_op_list(next_opt_val(rest, &mut i, "--pregrant")?)?);
                    }
                    s if s.starts_with("--pregrant=") => {
                        pregrant.extend(parse_op_list(&s["--pregrant=".len()..])?);
                    }
                    "--gated" => {
                        gated.extend(parse_op_list(next_opt_val(rest, &mut i, "--gated")?)?);
                    }
                    s if s.starts_with("--gated=") => {
                        gated.extend(parse_op_list(&s["--gated=".len()..])?);
                    }
                    s if label.is_none() => label = Some(s.to_string()),
                    other => return Err(usage_err(format!("unexpected argument '{other}'"))),
                }
                i += 1;
            }
            Ok(Some(PermissionsCmd::Register {
                label,
                pregrant,
                gated,
            }))
        }
        other => Err(usage_err(format!(
            "unknown permissions subcommand '{other}'"
        ))),
    }
}

#[derive(Debug)]
pub enum Command {
    Run,
    Calc {
        expression: String,
    },
    Query {
        query: String,
    },
    Display {
        command: Option<DisplayCmd>,
    },
    Window {
        command: Option<WindowCmd>,
    },
    Workspace {
        command: Option<WorkspaceCmd>,
    },
    Notification {
        command: Option<NotificationCmd>,
    },
    Journal {
        command: Option<JournalCmd>,
    },
    InteractionDomain {
        command: Option<InteractionDomainCmd>,
    },
    Permissions {
        command: Option<PermissionsCmd>,
    },
    Config {
        command: Option<ConfigCmd>,
    },
    System {
        command: Option<SystemCmd>,
    },
    Overview,
    Events,
    Completions {
        shell: Shell,
    },
    Quit,
}

#[derive(Debug)]
pub enum ConfigCmd {
    Validate { path: Option<PathBuf> },
    Migrate { path: Option<PathBuf> },
}

#[derive(Debug)]
pub enum DisplayCmd {
    List,
    Capture {
        path: Option<String>,
        region: Option<Region>,
    },
    Virtual {
        command: VirtualDisplayCmd,
    },
    Confirm,
    Topology {
        topology: TopologyArg,
    },
    Role {
        connector: String,
        role: RoleArg,
        source: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TopologyArg {
    Auto,
    Extend,
    Mirror,
}

impl FromStr for TopologyArg {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "auto" => Ok(Self::Auto),
            "extend" => Ok(Self::Extend),
            "mirror" => Ok(Self::Mirror),
            _ => Err(format!(
                "invalid topology '{s}'; expected auto, extend, or mirror"
            )),
        }
    }
}

impl From<TopologyArg> for tessera_desktop::output::DisplayTopology {
    fn from(value: TopologyArg) -> Self {
        match value {
            TopologyArg::Auto => Self::Auto,
            TopologyArg::Extend => Self::Extend,
            TopologyArg::Mirror => Self::Mirror,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoleArg {
    Automatic,
    Extend,
    Mirror,
}

impl FromStr for RoleArg {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "automatic" => Ok(Self::Automatic),
            "extend" => Ok(Self::Extend),
            "mirror" => Ok(Self::Mirror),
            _ => Err(format!(
                "invalid role '{s}'; expected automatic, extend, or mirror"
            )),
        }
    }
}

#[derive(Debug)]
pub enum VirtualDisplayCmd {
    Create {
        connector: Option<String>,
        mode: String,
        scale: f32,
        x: Option<i32>,
        y: Option<i32>,
    },
    Resize {
        connector: String,
        mode: String,
        scale: Option<f32>,
        x: Option<i32>,
        y: Option<i32>,
    },
    Destroy {
        connector: String,
    },
}

#[derive(Debug)]
pub enum WindowCmd {
    List,
    Focus {
        id: u64,
    },
    Minimize {
        id: u64,
    },
    AlwaysOnTop {
        id: u64,
        state: OnOff,
    },
    Fullscreen {
        id: u64,
        state: OnOff,
    },
    Close {
        id: u64,
    },
    Geometry {
        id: u64,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    },
}

#[derive(Debug)]
pub enum WorkspaceCmd {
    List,
    Switch { target: WorkspaceTarget },
    MoveWindow { window: u64, workspace: u64 },
}

#[derive(Debug)]
pub enum NotificationCmd {
    List,
    Send {
        summary: String,
        body: Option<String>,
    },
    Dismiss {
        id: u64,
    },
}

#[derive(Debug)]
pub enum JournalCmd {
    List {
        since: Option<u64>,
        topics: Vec<tessera_protocol::EventTopic>,
    },
    Follow {
        topics: Vec<tessera_protocol::EventTopic>,
    },
}

#[derive(Debug)]
pub enum SystemCmd {
    Status,
    Mute,
    StepVolume { delta: i8 },
    Volume { level: u8 },
    AudioOutput { name: String },
    MicMute,
    MicVolume { level: u8 },
    AudioInput { name: String },
    Brightness { level: u8 },
    Wifi { state: OnOff },
    Bluetooth { state: OnOff },
    DoNotDisturb { state: OnOff },
    PowerMode { mode: PowerModeArg },
    Suspend,
    Reboot,
    PowerOff,
}

#[derive(Debug, Clone, Copy)]
pub enum PowerModeArg {
    Balanced,
    Awake,
    Secure,
}

impl FromStr for PowerModeArg {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "balanced" => Ok(Self::Balanced),
            "awake" => Ok(Self::Awake),
            "secure" => Ok(Self::Secure),
            _ => Err(format!(
                "invalid power mode '{s}'; expected balanced, awake, or secure"
            )),
        }
    }
}

impl From<PowerModeArg> for tessera_desktop::power::PowerMode {
    fn from(value: PowerModeArg) -> Self {
        match value {
            PowerModeArg::Balanced => tessera_desktop::power::PowerMode::Balanced,
            PowerModeArg::Awake => tessera_desktop::power::PowerMode::Awake,
            PowerModeArg::Secure => tessera_desktop::power::PowerMode::Secure,
        }
    }
}

#[derive(Debug)]
pub enum InteractionDomainCmd {
    List,
    Create {
        label: String,
    },
    Pause {
        interaction_domain: u64,
    },
    Resume {
        interaction_domain: u64,
    },
    Transfer {
        window: u64,
        interaction_domain: u64,
        no_mirror: bool,
    },
    Launch {
        interaction_domain: u64,
        desktop_id: String,
    },
    Capture {
        interaction_domain: u64,
        path: Option<String>,
        region: Option<Region>,
    },
    Revoke {
        interaction_domain: u64,
        fallback: u64,
    },
}

#[derive(Debug)]
pub enum PermissionsCmd {
    List,
    Revoke {
        principal: String,
        op: tessera_protocol::ActorCapability,
    },
    Forget {
        principal: String,
    },
    Rename {
        principal: String,
        label: Option<String>,
    },
    SetCeiling {
        principal: String,
        pregrant: Vec<tessera_protocol::ActorCapability>,
        gated: Vec<tessera_protocol::ActorCapability>,
    },
    Register {
        label: Option<String>,
        pregrant: Vec<tessera_protocol::ActorCapability>,
        gated: Vec<tessera_protocol::ActorCapability>,
    },
}

fn op_class(value: &str) -> Result<tessera_protocol::ActorCapability, String> {
    tessera_protocol::ActorCapability::from_name(value)
        .ok_or_else(|| format!("unknown operation '{value}'"))
}

#[derive(Debug, Clone, Copy)]
pub enum WorkspaceTarget {
    Next,
    Previous,
    Id(u64),
}

impl FromStr for WorkspaceTarget {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "next" => Ok(Self::Next),
            "prev" | "previous" => Ok(Self::Previous),
            value => value.parse::<u64>().map(Self::Id).map_err(|_| {
                format!("invalid workspace target '{value}'; expected next, prev, or an id")
            }),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum OnOff {
    On,
    Off,
}

impl FromStr for OnOff {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "on" => Ok(Self::On),
            "off" => Ok(Self::Off),
            _ => Err(format!("invalid state '{s}'; expected on or off")),
        }
    }
}

impl From<OnOff> for bool {
    fn from(value: OnOff) -> Self {
        matches!(value, OnOff::On)
    }
}

impl From<WorkspaceTarget> for tessera_desktop::workspace::Switch {
    fn from(value: WorkspaceTarget) -> Self {
        match value {
            WorkspaceTarget::Next => tessera_desktop::workspace::Switch::Next,
            WorkspaceTarget::Previous => tessera_desktop::workspace::Switch::Prev,
            WorkspaceTarget::Id(_) => {
                unreachable!("concrete workspace ids do not use adjacent switching")
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Region(pub Rect);

impl FromStr for Region {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = value.split(',').collect();
        if parts.len() != 4 {
            return Err(format!("invalid region '{value}'; expected x,y,w,h"));
        }
        let mut numbers = [0i32; 4];
        for (index, part) in parts.into_iter().enumerate() {
            numbers[index] = part
                .parse()
                .map_err(|_| format!("invalid region '{value}'; expected integer x,y,w,h"))?;
        }
        if numbers[2] <= 0 || numbers[3] <= 0 {
            return Err("region width and height must be positive".into());
        }
        Ok(Region(Rect::new(
            numbers[0], numbers[1], numbers[2], numbers[3],
        )))
    }
}

impl From<Region> for Rect {
    fn from(value: Region) -> Self {
        value.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn journal_cli_parses_topic_filters_and_rejects_removed_audit_command() {
        let cli = Cli::try_parse_from([
            "tessera",
            "journal",
            "list",
            "--since",
            "10",
            "--topic",
            "mcp",
            "--topic=interaction-domain",
        ])
        .unwrap();
        match cli.command {
            Some(Command::Journal {
                command: Some(JournalCmd::List { since, topics }),
            }) => {
                assert_eq!(since, Some(10));
                assert_eq!(
                    topics,
                    vec![
                        tessera_protocol::EventTopic::Mcp,
                        tessera_protocol::EventTopic::InteractionDomain,
                    ]
                );
            }
            other => panic!("unexpected parsed command: {other:?}"),
        }

        let follow =
            Cli::try_parse_from(["tessera", "journal", "follow", "--topic", "window"]).unwrap();
        match follow.command {
            Some(Command::Journal {
                command: Some(JournalCmd::Follow { topics }),
            }) => {
                assert_eq!(topics, vec![tessera_protocol::EventTopic::Window]);
            }
            other => panic!("unexpected follow command: {other:?}"),
        }

        assert!(Cli::try_parse_from(["tessera", "journal", "list", "--topic", "bogus"]).is_err());
        assert!(Cli::try_parse_from(["tessera", "audit", "status"]).is_err());
    }
}
