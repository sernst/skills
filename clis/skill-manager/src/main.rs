#![forbid(unsafe_code)]
//! Executable boundary for skill-manager.

use std::fs;
use std::io;
use std::process::ExitCode;

use clap::{CommandFactory, Parser};
use clap_complete::Shell;
use skill_manager::app::{Application, RunOutcome, production_repository};
use skill_manager::cache::ReqwestGitHubTransport;
use skill_manager::cli::{Cli, Command, CompletionShell, StatusArgs};
use skill_manager::error::SkillManagerError;
use skill_manager::event::{ConsoleReporter, Level, Reporter};
use skill_manager::prompt::StdioPrompt;
use skill_manager::recipe::apply_recipe;
use skill_manager::transaction::NoopTransactionHook;

fn main() -> ExitCode {
    let mut cli = Cli::parse();
    let machine_mode = cli.machine_mode();
    let mut reporter = ConsoleReporter::with_color_policy(machine_mode, cli.color);
    if let Err(error) = apply_recipe(&mut cli) {
        report_error(&mut reporter, &error);
        return ExitCode::FAILURE;
    }
    let command = cli
        .command
        .take()
        .unwrap_or_else(|| Command::Status(StatusArgs::default()));
    if let Some(outcome) = run_generation(&command, &mut reporter) {
        return outcome;
    }
    let (repository, home) = match production_repository() {
        Ok(value) => value,
        Err(error) => {
            report_error(&mut reporter, &error);
            return ExitCode::FAILURE;
        }
    };
    let github = match ReqwestGitHubTransport::new() {
        Ok(value) => value,
        Err(error) => {
            report_error(&mut reporter, &error);
            return ExitCode::FAILURE;
        }
    };
    let mut prompt = StdioPrompt;
    let hook = NoopTransactionHook;
    let no_input = cli.no_input || machine_mode;
    let mut application = Application::new(
        &repository,
        &github,
        &mut prompt,
        &mut reporter,
        &hook,
        no_input,
        home,
    );
    match application.run(command) {
        Ok(RunOutcome::Success | RunOutcome::Cancelled) | Err(SkillManagerError::Cancelled) => {
            ExitCode::SUCCESS
        }
        Err(error) => {
            report_error(&mut reporter, &error);
            ExitCode::FAILURE
        }
    }
}

fn run_generation(command: &Command, reporter: &mut ConsoleReporter) -> Option<ExitCode> {
    match command {
        Command::GenerateCompletions(args) => {
            let shell = match args.shell {
                CompletionShell::Bash => Shell::Bash,
                CompletionShell::Zsh => Shell::Zsh,
                CompletionShell::Fish => Shell::Fish,
                CompletionShell::Powershell => Shell::PowerShell,
            };
            let mut command = Cli::command();
            clap_complete::generate(shell, &mut command, "skill-manager", &mut io::stdout());
            Some(ExitCode::SUCCESS)
        }
        Command::GenerateMan(args) => {
            let mut output = Vec::new();
            let result = clap_mangen::Man::new(Cli::command())
                .render(&mut output)
                .map_err(|error| SkillManagerError::io(&args.output, error))
                .and_then(|()| {
                    if let Some(parent) = args.output.parent() {
                        fs::create_dir_all(parent)
                            .map_err(|error| SkillManagerError::io(parent, error))?;
                    }
                    fs::write(&args.output, output)
                        .map_err(|error| SkillManagerError::io(&args.output, error))
                });
            if let Err(error) = result {
                report_error(reporter, &error);
                Some(ExitCode::FAILURE)
            } else {
                Some(ExitCode::SUCCESS)
            }
        }
        _ => None,
    }
}

fn report_error(reporter: &mut ConsoleReporter, error: &SkillManagerError) {
    if reporter.is_json() {
        let _result = reporter.event(
            "command.failed",
            Level::Error,
            serde_json::json!({ "message": error.to_string() }),
        );
    } else {
        let _result = reporter.diagnostic(&format!("Error: {error}"));
    }
}
