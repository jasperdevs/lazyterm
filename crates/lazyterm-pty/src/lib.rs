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
        let program = if cfg!(windows) {
            default_windows_shell_program(env::var("COMSPEC").ok())
        } else {
            default_unix_shell_program(env::var("SHELL").ok())
        };

        Self {
            program,
            args: Vec::new(),
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

fn default_windows_shell_program(comspec: Option<String>) -> String {
    comspec.unwrap_or_else(|| "cmd.exe".into())
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
        assert_eq!(default_windows_shell_program(None), "cmd.exe");
        assert_eq!(
            default_windows_shell_program(Some("custom.exe".into())),
            "custom.exe"
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
