//! Interactive input boundary.

use std::io::{self, BufRead, IsTerminal, Write};

use requestty::prompt::{
    Backend,
    backend::{ClearType, CrosstermBackend, DisplayBackend, MoveDirection, Size},
    events::CrosstermEvents,
    style::{Attributes, Color},
};
use requestty::{Answer, ErrorKind as RequesttyError, OnEsc, Question};

use crate::error::{Result, SkillManagerError};

const MAX_SELECTION_ATTEMPTS: usize = 4;

/// One selectable row in a radio or checklist prompt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PromptChoice {
    /// Stable, single-token line-mode selector.
    pub token: String,
    /// Human-facing label. Consequence previews belong in the rendered plan,
    /// not in this editor row.
    pub label: String,
    /// Initial checklist state. Radio prompts ignore this field.
    pub selected: bool,
}

impl PromptChoice {
    /// Build an unselected choice.
    #[must_use]
    pub fn new(token: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            token: token.into(),
            label: label.into(),
            selected: false,
        }
    }

    /// Set the initial checklist state.
    #[must_use]
    pub const fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }
}

/// Result of an interactive selection editor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PromptOutcome<T> {
    /// The user submitted a semantic selection. An empty checklist is valid.
    Submitted(T),
    /// Escape, Ctrl-C, or the explicit cancel token cancelled the editor.
    Cancelled,
}

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
    /// Select exactly one row, or cancel without authorizing it.
    ///
    /// The default line implementation keeps existing prompt doubles source
    /// compatible. Production terminals override it with a radio widget.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid option definitions, failed I/O, or a
    /// repeatedly unusable input stream.
    fn select_one(
        &mut self,
        message: &str,
        choices: &[PromptChoice],
    ) -> Result<PromptOutcome<usize>> {
        select_one_lines(self, message, choices)
    }
    /// Toggle zero or more rows and submit the resulting checklist.
    ///
    /// Each line-mode answer is one token: an option token toggles one row,
    /// `d` submits, and `c` cancels. Empty submission is valid.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid option definitions, failed I/O, or a
    /// repeatedly unusable input stream.
    fn select_many(
        &mut self,
        message: &str,
        choices: &[PromptChoice],
    ) -> Result<PromptOutcome<Vec<usize>>> {
        validate_choices(choices)?;
        let mut selected = choices
            .iter()
            .map(|choice| choice.selected)
            .collect::<Vec<_>>();
        edit_checklist(self, message, choices, &mut selected)
    }
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
#[derive(Clone, Copy, Debug)]
pub struct StdioPrompt {
    plain: bool,
    color: bool,
}

impl Default for StdioPrompt {
    fn default() -> Self {
        Self::new(false, true)
    }
}

impl StdioPrompt {
    /// Build the production prompt adapter.
    #[must_use]
    pub const fn new(plain: bool, color: bool) -> Self {
        Self { plain, color }
    }

    fn widgets_enabled(self) -> bool {
        widget_eligible(
            self.plain,
            [
                io::stdin().is_terminal(),
                io::stdout().is_terminal(),
                io::stderr().is_terminal(),
            ],
            std::env::var("TERM").is_ok_and(|term| term.eq_ignore_ascii_case("dumb")),
        )
    }

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

fn widget_eligible(plain: bool, terminal_streams: [bool; 3], term_is_dumb: bool) -> bool {
    // Crossterm's Unix cursor-position query uses stdout even when Requestty's
    // rendering backend writes to stderr, so every standard stream must be a
    // terminal before the widget may safely take terminal control.
    !plain && terminal_streams.into_iter().all(|is_terminal| is_terminal) && !term_is_dumb
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

    fn select_one(
        &mut self,
        message: &str,
        choices: &[PromptChoice],
    ) -> Result<PromptOutcome<usize>> {
        validate_choices(choices)?;
        if !self.widgets_enabled() {
            return select_one_lines(self, message, choices);
        }
        let labels = std::iter::once("Cancel".to_owned())
            .chain(
                choices
                    .iter()
                    .map(|choice| format!("{}  {}", choice.token, choice.label)),
            )
            .collect::<Vec<_>>();
        let question = Question::select("selection")
            .message(message)
            .choices(labels)
            .default(0)
            .should_loop(false)
            .on_esc(OnEsc::Terminate)
            .build();
        match ask_widget(question, self.color)? {
            PromptOutcome::Cancelled => Ok(PromptOutcome::Cancelled),
            PromptOutcome::Submitted(Answer::ListItem(item)) if item.index == 0 => {
                Ok(PromptOutcome::Cancelled)
            }
            PromptOutcome::Submitted(Answer::ListItem(item)) if item.index <= choices.len() => {
                Ok(PromptOutcome::Submitted(item.index - 1))
            }
            PromptOutcome::Submitted(Answer::ListItem(_)) => Err(SkillManagerError::InvalidInput(
                "radio prompt returned an out-of-range option".into(),
            )),
            PromptOutcome::Submitted(_) => Err(SkillManagerError::InvalidInput(
                "radio prompt returned an unexpected answer".into(),
            )),
        }
    }

    fn select_many(
        &mut self,
        message: &str,
        choices: &[PromptChoice],
    ) -> Result<PromptOutcome<Vec<usize>>> {
        validate_choices(choices)?;
        if !self.widgets_enabled() {
            let mut selected = choices
                .iter()
                .map(|choice| choice.selected)
                .collect::<Vec<_>>();
            return edit_checklist_stdio(message, choices, &mut selected);
        }
        let question = choices.iter().fold(
            Question::multi_select("selection")
                .message(message)
                .should_loop(false)
                .on_esc(OnEsc::Terminate),
            |builder, choice| {
                builder.choice_with_default(
                    format!("{}  {}", choice.token, choice.label),
                    choice.selected,
                )
            },
        );
        match ask_widget(question.build(), self.color)? {
            PromptOutcome::Cancelled => Ok(PromptOutcome::Cancelled),
            PromptOutcome::Submitted(Answer::ListItems(items)) => {
                let mut selected = items.into_iter().map(|item| item.index).collect::<Vec<_>>();
                selected.sort_unstable();
                if selected.iter().any(|index| *index >= choices.len())
                    || selected.windows(2).any(|pair| pair[0] == pair[1])
                {
                    return Err(SkillManagerError::InvalidInput(
                        "checklist prompt returned duplicate or out-of-range options".into(),
                    ));
                }
                Ok(PromptOutcome::Submitted(selected))
            }
            PromptOutcome::Submitted(_) => Err(SkillManagerError::InvalidInput(
                "checklist prompt returned an unexpected answer".into(),
            )),
        }
    }

    fn note(&mut self, message: &str) -> Result<()> {
        writeln!(io::stderr().lock(), "{message}")
            .map_err(|error| SkillManagerError::io("<stderr>", error))
    }
}

fn validate_choices(choices: &[PromptChoice]) -> Result<()> {
    if choices.is_empty() {
        return Err(SkillManagerError::InvalidInput(
            "a selection prompt needs at least one option".into(),
        ));
    }
    for (index, choice) in choices.iter().enumerate() {
        if choice.token.trim().is_empty() || choice.token.split_whitespace().count() != 1 {
            return Err(SkillManagerError::InvalidInput(
                "selection tokens must be non-empty single tokens".into(),
            ));
        }
        if choices[..index]
            .iter()
            .any(|other| other.token.eq_ignore_ascii_case(&choice.token))
        {
            return Err(SkillManagerError::InvalidInput(format!(
                "duplicate selection token '{}'",
                choice.token
            )));
        }
    }
    Ok(())
}

fn select_one_lines<P: Prompt + ?Sized>(
    prompt: &mut P,
    message: &str,
    choices: &[PromptChoice],
) -> Result<PromptOutcome<usize>> {
    validate_choices(choices)?;
    let hint = selection_hint(choices, false);
    for _ in 0..MAX_SELECTION_ATTEMPTS {
        let answer = prompt.exact_text(message)?.trim().to_owned();
        if answer.eq_ignore_ascii_case("c") {
            return Ok(PromptOutcome::Cancelled);
        }
        if let Some(index) = choices
            .iter()
            .position(|choice| choice.token.eq_ignore_ascii_case(&answer))
        {
            return Ok(PromptOutcome::Submitted(index));
        }
        prompt.note(&hint)?;
    }
    Err(SkillManagerError::InteractionRequired(format!(
        "no option was selected; {hint}"
    )))
}

fn selection_hint(choices: &[PromptChoice], checklist: bool) -> String {
    let tokens = choices
        .iter()
        .map(|choice| choice.token.clone())
        .collect::<Vec<_>>();
    if checklist {
        return format!("Enter {}, d to submit, or c to cancel.", tokens.join(", "));
    }
    let mut tokens = tokens;
    tokens.push("c".to_owned());
    let rendered = match tokens.as_slice() {
        [] => String::new(),
        [only] => only.clone(),
        [first, second] => format!("{first} or {second}"),
        [leading @ .., last] => format!("{}, or {last}", leading.join(", ")),
    };
    format!("Enter {rendered}.")
}

fn render_checklist<W: Write>(
    mut writer: W,
    choices: &[PromptChoice],
    selected: &[bool],
) -> Result<()> {
    for (choice, checked) in choices.iter().zip(selected) {
        let mark = if *checked { 'x' } else { ' ' };
        writeln!(writer, "  [{mark}] {}  {}", choice.token, choice.label)
            .map_err(|error| SkillManagerError::io("<stderr>", error))?;
    }
    Ok(())
}

fn edit_checklist<P: Prompt + ?Sized>(
    prompt: &mut P,
    message: &str,
    choices: &[PromptChoice],
    selected: &mut [bool],
) -> Result<PromptOutcome<Vec<usize>>> {
    let hint = selection_hint(choices, true);
    let mut invalid_answers = 0;
    loop {
        let answer = prompt.exact_text(message)?.trim().to_owned();
        if answer.eq_ignore_ascii_case("c") {
            return Ok(PromptOutcome::Cancelled);
        }
        if answer.eq_ignore_ascii_case("d") {
            return Ok(PromptOutcome::Submitted(selected_indices(selected)));
        }
        if let Some(index) = choices
            .iter()
            .position(|choice| choice.token.eq_ignore_ascii_case(&answer))
        {
            selected[index] = !selected[index];
        } else {
            prompt.note(&hint)?;
            invalid_answers += 1;
            if invalid_answers == MAX_SELECTION_ATTEMPTS {
                return Err(SkillManagerError::InteractionRequired(format!(
                    "the checklist was not submitted; {hint}"
                )));
            }
        }
    }
}

fn edit_checklist_stdio(
    message: &str,
    choices: &[PromptChoice],
    selected: &mut [bool],
) -> Result<PromptOutcome<Vec<usize>>> {
    let stdin = io::stdin();
    let mut reader = stdin.lock();
    let mut writer = io::stderr().lock();
    edit_checklist_lines(&mut reader, &mut writer, message, choices, selected)
}

fn edit_checklist_lines<R: BufRead, W: Write>(
    reader: &mut R,
    writer: &mut W,
    message: &str,
    choices: &[PromptChoice],
    selected: &mut [bool],
) -> Result<PromptOutcome<Vec<usize>>> {
    let hint = selection_hint(choices, true);
    let mut invalid_answers = 0;
    loop {
        writeln!(writer, "{message}").map_err(|error| SkillManagerError::io("<stderr>", error))?;
        render_checklist(&mut *writer, choices, selected)?;
        write!(writer, "Toggle selection [index, d done, c cancel]: ")
            .and_then(|()| writer.flush())
            .map_err(|error| SkillManagerError::io("<stderr>", error))?;
        let mut answer = String::new();
        let read = reader
            .read_line(&mut answer)
            .map_err(|error| SkillManagerError::io("<stdin>", error))?;
        if read == 0 {
            return Ok(PromptOutcome::Cancelled);
        }
        let answer = answer.trim();
        if answer.eq_ignore_ascii_case("c") {
            return Ok(PromptOutcome::Cancelled);
        }
        if answer.eq_ignore_ascii_case("d") {
            return Ok(PromptOutcome::Submitted(selected_indices(selected)));
        }
        if let Some(index) = choices
            .iter()
            .position(|choice| choice.token.eq_ignore_ascii_case(answer))
        {
            selected[index] = !selected[index];
        } else {
            writeln!(writer, "{hint}").map_err(|error| SkillManagerError::io("<stderr>", error))?;
            invalid_answers += 1;
            if invalid_answers == MAX_SELECTION_ATTEMPTS {
                return Err(SkillManagerError::InteractionRequired(format!(
                    "the checklist was not submitted; {hint}"
                )));
            }
        }
    }
}

fn selected_indices(selected: &[bool]) -> Vec<usize> {
    selected
        .iter()
        .enumerate()
        .filter_map(|(index, selected)| selected.then_some(index))
        .collect()
}

fn ask_widget(question: requestty::Question<'_>, color: bool) -> Result<PromptOutcome<Answer>> {
    let mut events = CrosstermEvents::new();
    let stderr = io::stderr();
    let backend = CrosstermBackend::new(stderr.lock());
    let result = if color {
        let mut backend = backend;
        requestty::prompt_one_with(question, &mut backend, &mut events)
    } else {
        let mut backend = ColorlessBackend(backend);
        requestty::prompt_one_with(question, &mut backend, &mut events)
    };
    map_widget_result(result)
}

fn map_widget_result(result: requestty::Result<Answer>) -> Result<PromptOutcome<Answer>> {
    match result {
        Ok(answer) => Ok(PromptOutcome::Submitted(answer)),
        Err(RequesttyError::Interrupted | RequesttyError::Aborted | RequesttyError::Eof) => {
            Ok(PromptOutcome::Cancelled)
        }
        Err(RequesttyError::IoError(error)) => Err(SkillManagerError::io("<terminal>", error)),
    }
}

struct ColorlessBackend<B>(B);

impl<B: Write> Write for ColorlessBackend<B> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.0.write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

impl<B: Backend> DisplayBackend for ColorlessBackend<B> {
    fn set_attributes(&mut self, _attributes: Attributes) -> io::Result<()> {
        Ok(())
    }

    fn set_fg(&mut self, _color: Color) -> io::Result<()> {
        Ok(())
    }

    fn set_bg(&mut self, _color: Color) -> io::Result<()> {
        Ok(())
    }
}

impl<B: Backend> Backend for ColorlessBackend<B> {
    fn enable_raw_mode(&mut self) -> io::Result<()> {
        self.0.enable_raw_mode()
    }
    fn disable_raw_mode(&mut self) -> io::Result<()> {
        self.0.disable_raw_mode()
    }
    fn hide_cursor(&mut self) -> io::Result<()> {
        self.0.hide_cursor()
    }
    fn show_cursor(&mut self) -> io::Result<()> {
        self.0.show_cursor()
    }
    fn get_cursor_pos(&mut self) -> io::Result<(u16, u16)> {
        self.0.get_cursor_pos()
    }
    fn move_cursor_to(&mut self, x: u16, y: u16) -> io::Result<()> {
        self.0.move_cursor_to(x, y)
    }
    fn move_cursor(&mut self, direction: MoveDirection) -> io::Result<()> {
        self.0.move_cursor(direction)
    }
    fn scroll(&mut self, distance: i16) -> io::Result<()> {
        self.0.scroll(distance)
    }
    fn clear(&mut self, clear_type: ClearType) -> io::Result<()> {
        self.0.clear(clear_type)
    }
    fn size(&self) -> io::Result<Size> {
        self.0.size()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::io::{self, Cursor, Write};

    use requestty::prompt::{
        Backend,
        backend::{ClearType, DisplayBackend, MoveDirection, Size, TestBackend},
        events::{KeyCode, KeyEvent, KeyModifiers, TestEvents},
        style::{Attributes, Color},
    };
    use requestty::{Answer, OnEsc, Question};

    use super::{
        ColorlessBackend, Prompt, PromptChoice, PromptOutcome, edit_checklist_lines,
        map_widget_result, parse_choice, parse_confirmation, resolve_text, validate_choices,
        widget_eligible,
    };
    use crate::error::{Result, SkillManagerError};

    #[derive(Default)]
    struct ScriptedPrompt {
        answers: VecDeque<String>,
        notes: Vec<String>,
    }

    impl Prompt for ScriptedPrompt {
        fn confirm(&mut self, _message: &str, default: bool) -> Result<bool> {
            Ok(default)
        }
        fn text(&mut self, _message: &str, default: Option<&str>) -> Result<String> {
            Ok(default.unwrap_or_default().to_owned())
        }
        fn exact_text(&mut self, _message: &str) -> Result<String> {
            Ok(self.answers.pop_front().unwrap_or_default())
        }
        fn choose(&mut self, _message: &str, _choices: &[String]) -> Result<usize> {
            Ok(0)
        }
        fn note(&mut self, message: &str) -> Result<()> {
            self.notes.push(message.to_owned());
            Ok(())
        }
    }

    fn choices() -> Vec<PromptChoice> {
        vec![
            PromptChoice::new("1", "Alpha"),
            PromptChoice::new("2", "Beta"),
            PromptChoice::new("3", "Gamma"),
        ]
    }

    struct TrackingBackend {
        inner: TestBackend,
        raw: bool,
        hidden: bool,
        style_calls: usize,
    }

    impl TrackingBackend {
        fn new(size: Size) -> Self {
            Self {
                inner: TestBackend::new(size),
                raw: false,
                hidden: false,
                style_calls: 0,
            }
        }
    }

    impl Write for TrackingBackend {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.inner.write(buffer)
        }
        fn flush(&mut self) -> io::Result<()> {
            self.inner.flush()
        }
    }

    impl DisplayBackend for TrackingBackend {
        fn set_attributes(&mut self, attributes: Attributes) -> io::Result<()> {
            self.style_calls += 1;
            self.inner.set_attributes(attributes)
        }
        fn set_fg(&mut self, color: Color) -> io::Result<()> {
            self.style_calls += 1;
            self.inner.set_fg(color)
        }
        fn set_bg(&mut self, color: Color) -> io::Result<()> {
            self.style_calls += 1;
            self.inner.set_bg(color)
        }
    }

    impl Backend for TrackingBackend {
        fn enable_raw_mode(&mut self) -> io::Result<()> {
            self.raw = true;
            self.inner.enable_raw_mode()
        }
        fn disable_raw_mode(&mut self) -> io::Result<()> {
            self.raw = false;
            self.inner.disable_raw_mode()
        }
        fn hide_cursor(&mut self) -> io::Result<()> {
            self.hidden = true;
            self.inner.hide_cursor()
        }
        fn show_cursor(&mut self) -> io::Result<()> {
            self.hidden = false;
            self.inner.show_cursor()
        }
        fn get_cursor_pos(&mut self) -> io::Result<(u16, u16)> {
            self.inner.get_cursor_pos()
        }
        fn move_cursor_to(&mut self, x: u16, y: u16) -> io::Result<()> {
            self.inner.move_cursor_to(x, y)
        }
        fn move_cursor(&mut self, direction: MoveDirection) -> io::Result<()> {
            self.inner.move_cursor(direction)
        }
        fn scroll(&mut self, distance: i16) -> io::Result<()> {
            self.inner.scroll(distance)
        }
        fn clear(&mut self, clear_type: ClearType) -> io::Result<()> {
            self.inner.clear(clear_type)
        }
        fn size(&self) -> io::Result<Size> {
            self.inner.size()
        }
    }

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

    #[test]
    fn semantic_radio_rejects_empty_and_out_of_range_then_selects() {
        let mut prompt = ScriptedPrompt {
            answers: VecDeque::from([String::new(), "9".into(), "2".into()]),
            ..ScriptedPrompt::default()
        };
        assert_eq!(
            prompt
                .select_one("Select", &choices())
                .unwrap_or(PromptOutcome::Cancelled),
            PromptOutcome::Submitted(1)
        );
        assert_eq!(
            prompt.notes,
            ["Enter 1, 2, 3, or c.", "Enter 1, 2, 3, or c."]
        );
    }

    #[test]
    fn semantic_checklist_toggles_without_duplicates_and_accepts_empty() {
        let mut prompt = ScriptedPrompt {
            answers: VecDeque::from(["1".into(), "1".into(), "d".into()]),
            ..ScriptedPrompt::default()
        };
        assert_eq!(
            prompt
                .select_many("Select inputs", &choices())
                .unwrap_or(PromptOutcome::Cancelled),
            PromptOutcome::Submitted(Vec::new())
        );
    }

    #[test]
    fn duplicate_and_malformed_tokens_are_rejected() {
        assert!(
            validate_choices(&[
                PromptChoice::new("one", "First"),
                PromptChoice::new("ONE", "Duplicate"),
            ])
            .is_err()
        );
        assert!(validate_choices(&[PromptChoice::new("two words", "Bad")]).is_err());
    }

    #[test]
    fn widget_requires_all_standard_streams_to_be_terminals() {
        assert!(widget_eligible(false, [true, true, true], false));
        assert!(!widget_eligible(false, [true, false, true], false));
        assert!(!widget_eligible(false, [false, true, true], false));
        assert!(!widget_eligible(false, [true, true, false], false));
        assert!(!widget_eligible(true, [true, true, true], false));
        assert!(!widget_eligible(false, [true, true, true], true));
    }

    #[test]
    fn piped_checklist_renders_controls_and_uses_one_token_commands() {
        let mut input = Cursor::new(b"9\n2\n1\nd\n");
        let mut output = Vec::new();
        let mut selected = vec![false; 3];
        let outcome = edit_checklist_lines(
            &mut input,
            &mut output,
            "Select inputs",
            &choices(),
            &mut selected,
        )
        .unwrap_or(PromptOutcome::Cancelled);
        assert_eq!(outcome, PromptOutcome::Submitted(vec![0, 1]));
        let rendered = String::from_utf8(output).unwrap_or_else(|error| unreachable!("{error}"));
        assert!(rendered.contains("[ ] 1  Alpha"));
        assert!(rendered.contains("[x] 2  Beta"));
        assert!(rendered.contains("Toggle selection [index, d done, c cancel]:"));
    }

    #[test]
    fn piped_checklist_stops_after_four_invalid_answers() {
        let mut input = Cursor::new(b"9\n9\n9\n9\nd\n");
        let mut output = Vec::new();
        let mut selected = vec![false; 3];
        let result = edit_checklist_lines(
            &mut input,
            &mut output,
            "Select inputs",
            &choices(),
            &mut selected,
        );
        assert!(matches!(
            result,
            Err(SkillManagerError::InteractionRequired(_))
        ));
    }

    #[test]
    fn widget_radio_uses_arrows_and_safe_cancel_row() {
        let question = Question::select("selection")
            .message("Select one")
            .choices(["Cancel", "1  Alpha", "2  Beta"])
            .default(0)
            .should_loop(false)
            .on_esc(OnEsc::Terminate)
            .build();
        let mut backend = TestBackend::new(Size {
            width: 60,
            height: 12,
        });
        let mut events = TestEvents::new([
            KeyCode::Down.into(),
            KeyCode::Down.into(),
            KeyCode::Enter.into(),
        ]);
        let answer = requestty::prompt_one_with(question, &mut backend, &mut events)
            .unwrap_or_else(|error| unreachable!("{error}"));
        let Answer::ListItem(item) = answer else {
            unreachable!("radio answer")
        };
        assert_eq!(item.index, 2);
        let rendered = backend.to_string();
        assert!(rendered.contains("Select one"));
        assert!(rendered.contains("2  Beta"));

        let question = Question::select("selection")
            .message("Select one")
            .choices(["Cancel", "1  Alpha", "2  Beta"])
            .default(0)
            .on_esc(OnEsc::Terminate)
            .build();
        let mut backend = TestBackend::new(Size {
            width: 60,
            height: 12,
        });
        let mut events = TestEvents::new([KeyCode::Enter.into()]);
        let answer = requestty::prompt_one_with(question, &mut backend, &mut events)
            .unwrap_or_else(|error| unreachable!("{error}"));
        let Answer::ListItem(item) = answer else {
            unreachable!("radio answer")
        };
        assert_eq!(
            item.index, 0,
            "Enter alone must resolve to the safe Cancel row"
        );
        assert!(backend.to_string().contains("Cancel"));
    }

    #[test]
    fn widget_checklist_handles_space_submit_escape_and_ctrl_c() {
        let question = Question::multi_select("selection")
            .message("Select inputs")
            .choices_with_default([("1  Alpha", false), ("2  Beta", false)])
            .should_loop(false)
            .on_esc(OnEsc::Terminate)
            .build();
        let mut backend = TestBackend::new(Size {
            width: 60,
            height: 12,
        });
        let mut events = TestEvents::new([
            KeyCode::Char(' ').into(),
            KeyCode::Down.into(),
            KeyCode::Char(' ').into(),
            KeyCode::Enter.into(),
        ]);
        let answer = requestty::prompt_one_with(question, &mut backend, &mut events)
            .unwrap_or_else(|error| unreachable!("{error}"));
        let Answer::ListItems(items) = answer else {
            unreachable!("checklist answer")
        };
        assert_eq!(
            items.iter().map(|item| item.index).collect::<Vec<_>>(),
            [0, 1]
        );
        assert!(backend.to_string().contains("Select inputs"));

        for event in [
            KeyCode::Esc.into(),
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        ] {
            let question = Question::multi_select("selection")
                .message("Cancel safely")
                .choice("Alpha")
                .on_esc(OnEsc::Terminate)
                .build();
            let mut backend = TrackingBackend::new(Size {
                width: 40,
                height: 8,
            });
            let mut events = TestEvents::new([event]);
            assert_eq!(
                map_widget_result(requestty::prompt_one_with(
                    question,
                    &mut backend,
                    &mut events,
                ))
                .unwrap_or(PromptOutcome::Submitted(Answer::Bool(false))),
                PromptOutcome::Cancelled
            );
            assert!(
                !backend.raw,
                "raw terminal mode must be restored after cancellation"
            );
            assert!(
                !backend.hidden,
                "the cursor must be restored after cancellation"
            );
        }
    }

    #[test]
    fn colorless_widget_backend_emits_no_ansi_sequences() {
        let question = Question::multi_select("selection")
            .message("Select inputs")
            .choice("Alpha")
            .build();
        let mut backend = ColorlessBackend(TrackingBackend::new(Size {
            width: 40,
            height: 8,
        }));
        let mut events = TestEvents::new([KeyCode::Char(' ').into(), KeyCode::Enter.into()]);
        let answer = requestty::prompt_one_with(question, &mut backend, &mut events)
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert!(matches!(answer, Answer::ListItems(_)));
        assert_eq!(backend.0.style_calls, 0);
    }
}
