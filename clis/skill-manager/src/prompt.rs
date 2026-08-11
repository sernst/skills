//! Interactive input boundary.

use std::io::{self, Write};

use crate::error::{Result, SkillManagerError};

/// Prompt boundary used by commands that need a human decision.
pub trait Prompt {
    /// Request a yes/no confirmation.
    ///
    /// # Errors
    ///
    /// Returns an error when input/output fails or the answer is invalid.
    fn confirm(&mut self, message: &str, default: bool) -> Result<bool>;
    /// Request one non-empty line of text, accepting an optional default.
    ///
    /// # Errors
    ///
    /// Returns an error when input/output fails or no value is supplied.
    fn text(&mut self, message: &str, default: Option<&str>) -> Result<String>;
    /// Request one line without normalizing or requiring a non-empty value.
    ///
    /// The default keeps test implementations source-compatible. Interactive
    /// implementations should override this to avoid presenting an empty
    /// bracketed default.
    ///
    /// # Errors
    ///
    /// Returns an error when input/output fails.
    fn exact_text(&mut self, message: &str) -> Result<String> {
        self.text(message, Some(""))
    }
    /// Choose an item by one-based index.
    ///
    /// # Errors
    ///
    /// Returns an error when input/output fails or the selection is invalid.
    fn choose(&mut self, message: &str, choices: &[String]) -> Result<usize>;
    /// Print guidance beside a prompt without asking anything.
    ///
    /// Used to reprompt after an unusable answer. The default is a no-op so
    /// test doubles stay source-compatible.
    ///
    /// # Errors
    ///
    /// Returns an error when output fails.
    fn note(&mut self, _message: &str) -> Result<()> {
        Ok(())
    }
}

/// Standard-input interactive prompt implementation.
#[derive(Clone, Copy, Debug, Default)]
pub struct StdioPrompt;

impl StdioPrompt {
    fn read_line_exact(message: &str) -> Result<String> {
        write!(io::stderr().lock(), "{message}")
            .and_then(|()| io::stderr().flush())
            .map_err(|error| SkillManagerError::io("<stderr>", error))?;
        let mut line = String::new();
        io::stdin()
            .read_line(&mut line)
            .map_err(|error| SkillManagerError::io("<stdin>", error))?;
        while line.ends_with('\r') || line.ends_with('\n') {
            line.pop();
        }
        Ok(line)
    }

    fn read_line(message: &str) -> Result<String> {
        Self::read_line_exact(message).map(|line| line.trim().to_owned())
    }
}

fn parse_confirmation(answer: &str, default: bool) -> Result<bool> {
    if answer.is_empty() {
        return Ok(default);
    }
    match answer.to_ascii_lowercase().as_str() {
        "y" | "yes" => Ok(true),
        "n" | "no" => Ok(false),
        _ => Err(SkillManagerError::InvalidInput(
            "expected 'yes' or 'no'".into(),
        )),
    }
}

fn resolve_text(answer: String, message: &str, default: Option<&str>) -> Result<String> {
    if answer.is_empty() {
        return default.map(ToOwned::to_owned).ok_or_else(|| {
            SkillManagerError::InvalidInput(format!("{message} must not be blank"))
        });
    }
    Ok(answer)
}

fn parse_choice(raw: &str, choice_count: usize) -> Result<usize> {
    let index = raw
        .parse::<usize>()
        .map_err(|_| SkillManagerError::InvalidInput("choice must be a number".into()))?;
    if index == 0 || index > choice_count {
        return Err(SkillManagerError::InvalidInput(format!(
            "choice must be between 1 and {choice_count}"
        )));
    }
    Ok(index - 1)
}

impl Prompt for StdioPrompt {
    fn confirm(&mut self, message: &str, default: bool) -> Result<bool> {
        let suffix = if default { " [Y/n] " } else { " [y/N] " };
        let answer = Self::read_line(&format!("{message}{suffix}"))?;
        parse_confirmation(&answer, default)
    }

    fn text(&mut self, message: &str, default: Option<&str>) -> Result<String> {
        let prompt = default.map_or_else(
            || format!("{message}: "),
            |value| format!("{message} [{value}]: "),
        );
        let answer = Self::read_line(&prompt)?;
        resolve_text(answer, message, default)
    }

    fn exact_text(&mut self, message: &str) -> Result<String> {
        Self::read_line_exact(&format!("{message}: "))
    }

    fn choose(&mut self, message: &str, choices: &[String]) -> Result<usize> {
        if choices.is_empty() {
            return Err(SkillManagerError::InvalidInput(
                "cannot choose from an empty list".into(),
            ));
        }
        writeln!(io::stderr().lock(), "{message}")
            .map_err(|error| SkillManagerError::io("<stderr>", error))?;
        for (index, choice) in choices.iter().enumerate() {
            writeln!(io::stderr().lock(), "  {}. {choice}", index + 1)
                .map_err(|error| SkillManagerError::io("<stderr>", error))?;
        }
        let raw = Self::read_line("Choice: ")?;
        parse_choice(&raw, choices.len())
    }

    fn note(&mut self, message: &str) -> Result<()> {
        writeln!(io::stderr().lock(), "{message}")
            .map_err(|error| SkillManagerError::io("<stderr>", error))
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_choice, parse_confirmation, resolve_text};

    #[test]
    fn confirmation_accepts_defaults_and_case_insensitive_answers() {
        assert!(parse_confirmation("", true).unwrap_or(false));
        assert!(!parse_confirmation("", false).unwrap_or(true));
        for answer in ["y", "Y", "yes", "YES"] {
            assert!(parse_confirmation(answer, false).unwrap_or(false));
        }
        for answer in ["n", "N", "no", "NO"] {
            assert!(!parse_confirmation(answer, true).unwrap_or(true));
        }
        assert!(parse_confirmation("perhaps", true).is_err());
    }

    #[test]
    fn text_accepts_an_answer_or_default_and_rejects_blank_without_one() {
        assert_eq!(
            resolve_text("typed".into(), "Name", Some("default"))
                .unwrap_or_else(|error| unreachable!("{error}")),
            "typed"
        );
        assert_eq!(
            resolve_text(String::new(), "Name", Some("default"))
                .unwrap_or_else(|error| unreachable!("{error}")),
            "default"
        );
        assert!(resolve_text(String::new(), "Name", None).is_err());
    }

    #[test]
    fn choice_is_one_based_and_strictly_bounded() {
        assert_eq!(
            parse_choice("1", 3).unwrap_or_else(|error| unreachable!("{error}")),
            0
        );
        assert_eq!(
            parse_choice("3", 3).unwrap_or_else(|error| unreachable!("{error}")),
            2
        );
        assert!(parse_choice("0", 3).is_err());
        assert!(parse_choice("4", 3).is_err());
        assert!(parse_choice("one", 3).is_err());
    }
}
