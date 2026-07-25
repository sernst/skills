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
}

/// Reporter backed by stdout and stderr.
pub struct ConsoleReporter {
    json: bool,
    color: bool,
    interactive: bool,
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
        let interactive = io::stdout().is_terminal();
        let no_color = std::env::var_os("NO_COLOR").is_some();
        let color = !json
            && !no_color
            && match policy {
                ColorChoice::Auto => interactive,
                ColorChoice::Always => true,
                ColorChoice::Never => false,
            };
        Self {
            json,
            color,
            interactive,
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
        if self.color {
            writeln!(io::stdout().lock(), "\u{1b}[36m{text}\u{1b}[0m")
        } else {
            writeln!(io::stdout().lock(), "{text}")
        }
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
}
