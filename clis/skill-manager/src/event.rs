//! Human and newline-delimited JSON reporting.

use std::io::{self, IsTerminal, Write};

use serde::Serialize;
use serde_json::Value;

use crate::cli::ColorChoice;
use crate::error::{Result, SkillManagerError};

/// Severity carried by a structured event.
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    /// Normal command data.
    Info,
    /// Recoverable or compatibility diagnostic.
    Warning,
    /// Command failure.
    Error,
}

/// Stable NDJSON envelope.
#[derive(Debug, Serialize)]
pub struct Event<'a> {
    /// Envelope schema version.
    pub version: u8,
    /// Event family name.
    pub event: &'a str,
    /// Severity.
    pub level: Level,
    /// Event-specific payload.
    pub data: Value,
}

/// Output boundary used by application services.
pub trait Reporter {
    /// Emit semantic command output.
    ///
    /// # Errors
    ///
    /// Returns an error when the output stream cannot be written or serialized.
    fn event(&mut self, event: &str, level: Level, data: Value) -> Result<()>;
    /// Emit human-readable data.
    ///
    /// # Errors
    ///
    /// Returns an error when standard output cannot be written.
    fn human(&mut self, text: &str) -> Result<()>;
    /// Emit bytes without modification.
    ///
    /// This is used only for recovery-oriented raw configuration output. The
    /// default implementation supports UTF-8 test reporters; production
    /// reporters override it so arbitrary bytes and trailing newlines are
    /// preserved exactly.
    ///
    /// # Errors
    ///
    /// Returns an error when the bytes are not UTF-8 or cannot be written.
    fn raw(&mut self, bytes: &[u8]) -> Result<()> {
        let text = std::str::from_utf8(bytes).map_err(|error| {
            SkillManagerError::InvalidInput(format!("raw output is not valid UTF-8: {error}"))
        })?;
        self.human(text)
    }
    /// Emit a human-readable diagnostic.
    ///
    /// # Errors
    ///
    /// Returns an error when standard error cannot be written.
    fn diagnostic(&mut self, text: &str) -> Result<()>;
    /// Whether machine-only output is active.
    fn is_json(&self) -> bool;
    /// Whether human output is attached to an interactive terminal.
    fn is_interactive(&self) -> bool {
        false
    }
    /// Whether semantic status cells may use ANSI colors.
    fn color_enabled(&self) -> bool {
        false
    }
    /// Whether advanced human-readable details were requested.
    fn verbose(&self) -> bool {
        false
    }
}

/// Reporter backed by stdout and stderr.
#[allow(clippy::struct_excessive_bools)]
pub struct ConsoleReporter {
    json: bool,
    color: bool,
    interactive: bool,
    verbose: bool,
}

impl ConsoleReporter {
    /// Create a console reporter.
    #[must_use]
    pub fn new(json: bool) -> Self {
        Self::with_color_policy(json, ColorChoice::Auto)
    }

    /// Create a reporter with an explicit CLI color policy.
    #[must_use]
    pub fn with_color_policy(json: bool, policy: ColorChoice) -> Self {
        Self::with_human_options(json, policy, false)
    }

    /// Environment override that forces the interactive symbol vocabulary on.
    ///
    /// Golden tests must be able to review the symbol rendering a terminal user
    /// sees, and a spawned test process never owns a TTY. This is a deliberate,
    /// narrow injection seam matching how `SKILL_MANAGER_HOME` redirects
    /// configuration, and `--color` remains the supported way to influence
    /// color.
    ///
    /// The override is one-directional on purpose. Opting a redirected stream
    /// into symbols is harmless and self-describing, but silently downgrading a
    /// real terminal is not, so any value other than `1` — including `0` and a
    /// stray leftover value — falls through to the real terminal probe rather
    /// than suppressing it.
    const FORCE_INTERACTIVE: &'static str = "SKILL_MANAGER_FORCE_INTERACTIVE";

    /// Create a reporter with explicit human-output options.
    #[must_use]
    pub fn with_human_options(json: bool, policy: ColorChoice, verbose: bool) -> Self {
        let forced = std::env::var_os(Self::FORCE_INTERACTIVE).is_some_and(|value| value == "1");
        let interactive = forced || io::stdout().is_terminal();
        let no_color = std::env::var_os("NO_COLOR").is_some();
        let color = !json
            && match policy {
                ColorChoice::Auto => interactive && !no_color,
                ColorChoice::Always => true,
                ColorChoice::Never => false,
            };
        Self {
            json,
            color,
            interactive,
            verbose,
        }
    }
}

impl Reporter for ConsoleReporter {
    fn event(&mut self, event: &str, level: Level, data: Value) -> Result<()> {
        if !self.json {
            return Ok(());
        }
        let record = Event {
            version: 1,
            event,
            level,
            data,
        };
        let line = serde_json::to_string(&record)
            .map_err(|error| SkillManagerError::InvalidInput(error.to_string()))?;
        writeln!(io::stdout().lock(), "{line}")
            .map_err(|error| SkillManagerError::io("<stdout>", error))
    }

    fn human(&mut self, text: &str) -> Result<()> {
        if self.json {
            return Ok(());
        }
        writeln!(io::stdout().lock(), "{text}")
            .map_err(|error| SkillManagerError::io("<stdout>", error))
    }

    fn raw(&mut self, bytes: &[u8]) -> Result<()> {
        if self.json {
            return Err(SkillManagerError::InvalidInput(
                "raw output cannot be combined with JSON output".into(),
            ));
        }
        io::stdout()
            .lock()
            .write_all(bytes)
            .map_err(|error| SkillManagerError::io("<stdout>", error))
    }

    fn diagnostic(&mut self, text: &str) -> Result<()> {
        if self.json {
            return Ok(());
        }
        if self.color {
            writeln!(io::stderr().lock(), "\u{1b}[31m{text}\u{1b}[0m")
        } else {
            writeln!(io::stderr().lock(), "{text}")
        }
        .map_err(|error| SkillManagerError::io("<stderr>", error))
    }

    fn is_json(&self) -> bool {
        self.json
    }

    fn is_interactive(&self) -> bool {
        self.interactive && !self.json
    }

    fn color_enabled(&self) -> bool {
        self.color
    }

    fn verbose(&self) -> bool {
        self.verbose && !self.json
    }
}
