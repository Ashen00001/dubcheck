//! dubcheck: a read-only audit of an anime library.
//!
//! The library crate holds all the logic; `main.rs` is a thin wrapper.

pub mod cache;
pub mod cli;
pub mod parser;
pub mod probe;
pub mod report;
pub mod scanner;
