//! `the skills list | show | dirs | lint | cleanup`. The skills **core logic** lives in the shared
//! [`the_skills`] crate; this module keeps the CLI command + rendering layer ([`cmd`], which renders
//! through [`crate::table`]) and re-exports the core so the command code reaches it unchanged.

pub mod cmd;

pub use the_skills::{
    Agent, Scope, analysis, bundled_files, fix, installed, locate, parse, resolve, rules,
};
