#![forbid(unsafe_code)]
//! Idiomatic Rust implementation of the skill-manager application.

/// Application service and command orchestration.
pub mod app;
/// GitHub source transport and persistent cache.
pub mod cache;
/// Command-line interface types.
pub mod cli;
/// Versioned configuration and source/target normalization.
pub mod config;
/// Core domain types.
pub mod domain;
/// Typed application errors.
pub mod error;
/// Human and machine reporting.
pub mod event;
/// Interactive input boundary.
pub mod prompt;
/// Strict JSON invocation input.
pub mod recipe;
/// Skill discovery and status helpers.
pub mod skills;
/// Human-readable status table rendering.
pub mod status;
/// Isolated migration from historical flat storage locations.
pub mod storage_migration;
/// Journaled filesystem transactions.
pub mod transaction;
