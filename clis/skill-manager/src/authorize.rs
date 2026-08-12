//! Plan authorization: the one place a rendered plan turns into consent.
//!
//! Authorization never introduces facts. Every question it asks is answered by
//! one token that already appears in the plan the user just reviewed, and every
//! question is resolvable by cancelling. Two shapes cover every command in the
//! approved design:
//!
//! * [`Authorizer::confirm`](crate::authorize::Authorizer::confirm) for
//!   exactly one complete outcome, where bracket casing (`[Y/n]` versus `[y/N]`)
//!   is the only encoding of risk.
//! * [`Authorizer::select`](crate::authorize::Authorizer::select) for exactly
//!   one unresolved dimension, where numbered options and an explicit cancel
//!   token are read off rendered option lines.
//!
//! A progressive, multi-prompt sequence is composed by calling
//! [`Authorizer`](crate::authorize::Authorizer) once per dimension and
//! re-rendering a narrowed plan in between; the helper deliberately holds no
//! cross-prompt state so that narrowing stays the caller's explicit, testable
//! step.

use crate::error::{Result, SkillManagerError};
use crate::prompt::Prompt;

/// Token that cancels any selection prompt.
pub const CANCEL_TOKEN: &str = "c";

/// Maximum invalid answers before a selection prompt gives up.
///
/// An invalid or empty answer must reprompt and must never select, but a closed
/// or non-interactive stream would otherwise loop forever.
const MAX_ATTEMPTS: usize = 4;

/// Outcome of one authorization step.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Authorization<T> {
    /// The user authorized the reviewed plan revision.
    Approved(T),
    /// The user cancelled; nothing may be written.
    Cancelled,
}

impl<T> Authorization<T> {
    /// Whether the user authorized the plan.
    #[must_use]
    pub const fn is_approved(&self) -> bool {
        matches!(self, Self::Approved(_))
    }
}

/// One rendered option line a selection prompt can resolve to.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SelectionOption {
    /// Single token printed on the option line, normally `1`, `2`, or `3`.
    pub token: String,
    /// Human-facing option label.
    pub label: String,
    /// Whether choosing this option deletes or overwrites authoritative content.
    pub destructive: bool,
}

impl SelectionOption {
    /// Build a one-based numbered option.
    #[must_use]
    pub fn numbered(index: usize, label: impl Into<String>, destructive: bool) -> Self {
        Self {
            token: (index + 1).to_string(),
            label: label.into(),
            destructive,
        }
    }
}

/// Authorization boundary layered over the interactive [`Prompt`] seam.
pub struct Authorizer<'a, P> {
    prompt: &'a mut P,
}

impl<'a, P: Prompt> Authorizer<'a, P> {
    /// Wrap the interactive prompt port.
    pub const fn new(prompt: &'a mut P) -> Self {
        Self { prompt }
    }

    /// Ask one binary question about exactly one complete outcome.
    ///
    /// `destructive` selects the default: additive or easily regenerated work
    /// defaults to yes, while deletion, authoritative overwrite, or adoption of
    /// external state defaults to no.
    ///
    /// # Errors
    ///
    /// Returns an error when the interactive stream fails or the answer is not
    /// a recognized yes/no value.
    pub fn confirm(&mut self, question: &str, destructive: bool) -> Result<Authorization<()>> {
        if self.prompt.confirm(question, !destructive)? {
            Ok(Authorization::Approved(()))
        } else {
            Ok(Authorization::Cancelled)
        }
    }

    /// Resolve exactly one dimension from one token on a rendered option line.
    ///
    /// No option is preselected when every option is destructive or
    /// authoritative, so pressing Enter reprompts and never authorizes. A
    /// recommendation is guidance printed with the plan, never consent.
    ///
    /// # Errors
    ///
    /// Returns an error when the option list is empty, the interactive stream
    /// fails, or the answer stays unusable after repeated reprompts.
    pub fn select(
        &mut self,
        question: &str,
        options: &[SelectionOption],
    ) -> Result<Authorization<usize>> {
        if options.is_empty() {
            return Err(SkillManagerError::InvalidInput(
                "a selection prompt needs at least one rendered option".into(),
            ));
        }
        let hint = reprompt_hint(options);
        for _ in 0..MAX_ATTEMPTS {
            let answer = self.prompt.exact_text(question)?.trim().to_owned();
            if answer.eq_ignore_ascii_case(CANCEL_TOKEN) {
                return Ok(Authorization::Cancelled);
            }
            if let Some(index) = options
                .iter()
                .position(|option| option.token.eq_ignore_ascii_case(&answer))
            {
                return Ok(Authorization::Approved(index));
            }
            self.prompt.note(&hint)?;
        }
        Err(SkillManagerError::InteractionRequired(format!(
            "no option was selected; {hint}"
        )))
    }
}

/// Render the bracketed token range printed in a selection question.
#[must_use]
pub fn selection_range(options: &[SelectionOption]) -> String {
    match options {
        [] => format!("{CANCEL_TOKEN} to cancel"),
        [only] => format!("{}, {CANCEL_TOKEN} to cancel", only.token),
        [first, .., last] => format!("{}-{}, {CANCEL_TOKEN} to cancel", first.token, last.token),
    }
}

/// Render the guidance printed after an invalid or empty selection answer.
#[must_use]
pub fn reprompt_hint(options: &[SelectionOption]) -> String {
    let mut tokens = options
        .iter()
        .map(|option| option.token.clone())
        .collect::<Vec<_>>();
    tokens.push(CANCEL_TOKEN.to_owned());
    let rendered = match tokens.as_slice() {
        [] => String::new(),
        [only] => only.clone(),
        [first, second] => format!("{first} or {second}"),
        [leading @ .., last] => format!("{}, or {last}", leading.join(", ")),
    };
    format!("Enter {rendered}.")
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::{Authorization, Authorizer, SelectionOption, reprompt_hint, selection_range};
    use crate::error::Result;
    use crate::prompt::Prompt;

    #[derive(Default)]
    struct ScriptedPrompt {
        answers: VecDeque<String>,
        confirmations: VecDeque<bool>,
        notes: Vec<String>,
        questions: Vec<String>,
    }

    impl Prompt for ScriptedPrompt {
        fn confirm(&mut self, message: &str, default: bool) -> Result<bool> {
            self.questions.push(message.to_owned());
            Ok(self.confirmations.pop_front().unwrap_or(default))
        }

        fn text(&mut self, _message: &str, default: Option<&str>) -> Result<String> {
            Ok(self
                .answers
                .pop_front()
                .or_else(|| default.map(ToOwned::to_owned))
                .unwrap_or_default())
        }

        fn exact_text(&mut self, message: &str) -> Result<String> {
            self.questions.push(message.to_owned());
            Ok(self.answers.pop_front().unwrap_or_default())
        }

        fn choose(&mut self, _message: &str, _choices: &[String]) -> Result<usize> {
            Ok(0)
        }

        fn note(&mut self, text: &str) -> Result<()> {
            self.notes.push(text.to_owned());
            Ok(())
        }
    }

    fn options() -> Vec<SelectionOption> {
        vec![
            SelectionOption::numbered(0, "Remove project copies", true),
            SelectionOption::numbered(1, "Remove global copies", true),
            SelectionOption::numbered(2, "Remove both copies", true),
        ]
    }

    #[test]
    fn binary_confirmation_defaults_follow_destructiveness() {
        let mut prompt = ScriptedPrompt::default();
        let approved = Authorizer::new(&mut prompt)
            .confirm("Apply this update plan to 3 enabled targets?", false)
            .unwrap_or(Authorization::Cancelled);
        assert!(approved.is_approved());

        let mut destructive = ScriptedPrompt::default();
        let declined = Authorizer::new(&mut destructive)
            .confirm("Remove these 2 deployments from 1 selected target?", true)
            .unwrap_or(Authorization::Approved(()));
        assert_eq!(declined, Authorization::Cancelled);
    }

    #[test]
    fn selection_reprompts_on_empty_or_invalid_answers_and_never_selects() {
        let mut prompt = ScriptedPrompt {
            answers: VecDeque::from([String::new(), "9".into(), "2".into()]),
            ..ScriptedPrompt::default()
        };
        let outcome = Authorizer::new(&mut prompt)
            .select("Select removal scope [1-3, c to cancel]", &options())
            .unwrap_or(Authorization::Cancelled);
        assert_eq!(outcome, Authorization::Approved(1));
        assert_eq!(
            prompt.notes,
            ["Enter 1, 2, 3, or c.", "Enter 1, 2, 3, or c."]
        );
    }

    #[test]
    fn selection_cancels_explicitly_and_gives_up_after_repeated_bad_answers() {
        let mut cancelled = ScriptedPrompt {
            answers: VecDeque::from(["c".into()]),
            ..ScriptedPrompt::default()
        };
        assert_eq!(
            Authorizer::new(&mut cancelled)
                .select("Select removal scope [1-3, c to cancel]", &options())
                .unwrap_or(Authorization::Approved(0)),
            Authorization::Cancelled
        );

        let mut exhausted = ScriptedPrompt::default();
        assert!(
            Authorizer::new(&mut exhausted)
                .select("Select removal scope [1-3, c to cancel]", &options())
                .is_err()
        );
    }

    #[test]
    fn rendered_option_ranges_and_hints_match_the_prompt_copy() {
        assert_eq!(selection_range(&options()), "1-3, c to cancel");
        assert_eq!(selection_range(&options()[..1]), "1, c to cancel");
        assert_eq!(reprompt_hint(&options()), "Enter 1, 2, 3, or c.");
        assert_eq!(reprompt_hint(&options()[..1]), "Enter 1 or c.");
    }
}
