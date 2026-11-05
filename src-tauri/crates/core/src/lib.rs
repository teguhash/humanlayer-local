//! Domain types, SQLite storage and the login-shell environment.

pub mod db;
pub mod model;
pub mod shell_env;

pub use db::{Db, Error};
