use std::env;
use std::io::{Read, Write};
use std::path::PathBuf;

use anyhow::Result;
use lazyterm_terminal::TerminalSize;
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShellCommand {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
}

impl ShellCommand {
    pub fn default_for_platform() -> Self {
        let (program, args) = if cfg!(windows) {
            default_windows_shell(env::var("COMSPEC").ok())
        } else {
            (
                default_unix_shell_program(env::var("SHELL").ok()),
                Vec::new(),
            )
        };

        Self {
            program,
            args,
            cwd: None,
        }
    }

    pub fn command_builder(&self) -> CommandBuilder {
        let mut builder = CommandBuilder::new(&self.program);
        builder.args(&self.args);

        if let Some(cwd) = &self.cwd {
            builder.cwd(cwd);
        }

        builder
    }
}

fn default_windows_shell(comspec: Option<String>) -> (String, Vec<String>) {
    default_windows_shell_with(
        comspec,
        shell_program_on_path("pwsh.exe"),
        shell_program_on_path("powershell.exe"),
    )
}

fn default_windows_shell_with(
    comspec: Option<String>,
    has_pwsh: bool,
    has_powershell: bool,
) -> (String, Vec<String>) {
    if has_pwsh {
        return (
            "pwsh.exe".into(),
            vec!["-NoLogo".into(), "-NoProfile".into()],
        );
    }

    if has_powershell {
        return (
            "powershell.exe".into(),
            vec!["-NoLogo".into(), "-NoProfile".into()],
        );
    }

    (comspec.unwrap_or_else(|| "cmd.exe".into()), Vec::new())
}

fn shell_program_on_path(program: &str) -> bool {
    let Some(path) = env::var_os("PATH") else {
        return false;
    };

    env::split_paths(&path).any(|directory| directory.join(program).is_file())
}

fn default_unix_shell_program(shell: Option<String>) -> String {
    shell.unwrap_or_else(|| "/bin/sh".into())
}

pub fn terminal_size_to_pty_size(size: TerminalSize) -> PtySize {
    PtySize {
        rows: size.rows,
        cols: size.columns,
        pixel_width: 0,
        pixel_height: 0,
    }
}

pub struct PtySession {
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    reader: Box<dyn Read + Send>,
    writer: Box<dyn Write + Send>,
}

pub struct PtyHandle {
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    writer: Box<dyn Write + Send>,
}

pub struct PtyReader {
    reader: Box<dyn Read + Send>,
}

impl PtySession {
    pub fn spawn(command: &ShellCommand, size: impl Into<PtySize>) -> Result<Self> {
        let pty_system = native_pty_system();
        let pair = pty_system.openpty(size.into())?;
        let child = pair.slave.spawn_command(command.command_builder())?;
        let reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;

        Ok(Self {
            master: pair.master,
            child,
            reader,
            writer,
        })
    }

    pub fn resize(&self, size: impl Into<PtySize>) -> Result<()> {
        self.master.resize(size.into())
    }

    pub fn write_all(&mut self, bytes: &[u8]) -> Result<()> {
        self.writer.write_all(bytes)?;
        self.writer.flush()?;
        Ok(())
    }

    pub fn reader(&mut self) -> &mut dyn Read {
        &mut *self.reader
    }

    pub fn try_wait(&mut self) -> std::io::Result<Option<portable_pty::ExitStatus>> {
        self.child.try_wait()
    }

    pub fn kill(&mut self) -> std::io::Result<()> {
        self.child.kill()
    }

    pub fn split(self) -> (PtyHandle, PtyReader) {
        (
            PtyHandle {
                master: self.master,
                child: self.child,
                writer: self.writer,
            },
            PtyReader {
                reader: self.reader,
            },
        )
    }
}

impl PtyHandle {
    pub fn resize(&self, size: impl Into<PtySize>) -> Result<()> {
        self.master.resize(size.into())
    }

    pub fn write_all(&mut self, bytes: &[u8]) -> Result<()> {
        self.writer.write_all(bytes)?;
        self.writer.flush()?;
        Ok(())
    }

    pub fn try_wait(&mut self) -> std::io::Result<Option<portable_pty::ExitStatus>> {
        self.child.try_wait()
    }

    pub fn kill(&mut self) -> std::io::Result<()> {
        self.child.kill()
    }
}

impl Drop for PtyHandle {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

impl PtyReader {
    pub fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.reader.read(buffer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_default_shell_is_not_empty() {
        assert!(!ShellCommand::default_for_platform().program.is_empty());
    }

    #[test]
    fn builds_command_with_args_and_cwd() {
        let command = ShellCommand {
            program: "program".into(),
            args: vec!["--flag".into(), "value".into()],
            cwd: Some(PathBuf::from("C:/work")),
        };
        let builder = command.command_builder();

        assert_eq!(builder.get_argv().len(), 3);
        assert_eq!(builder.get_argv()[0], "program");
        assert_eq!(builder.get_argv()[1], "--flag");
        assert_eq!(builder.get_argv()[2], "value");
        assert_eq!(
            builder
                .get_cwd()
                .map(|cwd| cwd.to_string_lossy().into_owned()),
            Some("C:/work".into())
        );
    }

    #[test]
    fn terminal_size_converts_into_pty_size() {
        let size = terminal_size_to_pty_size(TerminalSize::new(80, 24));

        assert_eq!(size.cols, 80);
        assert_eq!(size.rows, 24);
        assert_eq!(size.pixel_width, 0);
        assert_eq!(size.pixel_height, 0);
    }

    #[cfg(windows)]
    #[test]
    fn windows_default_shell_falls_back_to_cmd() {
        assert_eq!(
            default_windows_shell_with(None, false, false),
            ("cmd.exe".into(), Vec::new())
        );
        assert_eq!(
            default_windows_shell_with(Some("custom.exe".into()), false, false),
            ("custom.exe".into(), Vec::new())
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_default_shell_prefers_powershell() {
        assert_eq!(
            default_windows_shell_with(Some("cmd.exe".into()), true, true),
            (
                "pwsh.exe".into(),
                vec!["-NoLogo".into(), "-NoProfile".into()]
            )
        );
        assert_eq!(
            default_windows_shell_with(Some("cmd.exe".into()), false, true),
            (
                "powershell.exe".into(),
                vec!["-NoLogo".into(), "-NoProfile".into()]
            )
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn unix_default_shell_falls_back_to_sh() {
        assert_eq!(default_unix_shell_program(None), "/bin/sh");
        assert_eq!(
            default_unix_shell_program(Some("/bin/zsh".into())),
            "/bin/zsh"
        );
    }
}
