//! Core of SQLx, the rust SQL toolkit.
//!
//! ### Note: Semver Exempt API
//! The API of this crate is not meant for general use and does *not* follow Semantic Versioning.
//! The only crate that follows Semantic Versioning in the project is the `sqlx` crate itself.
//! If you are building a custom SQLx driver, you should pin an exact version for `sqlx-core` to
//! avoid breakages:
//!
//! ```toml
//! sqlx-core = { version = "=0.6.2" }
//! ```
//!
//! And then make releases in lockstep with `sqlx-core`. We recommend all driver crates, in-tree
//! or otherwise, use the same version numbers as `sqlx-core` to avoid confusion.
#![recursion_limit = "512"]
#![warn(future_incompatible, rust_2018_idioms)]
#![allow(clippy::needless_doctest_main, clippy::type_complexity)]
// The only unsafe code in SQLx is that necessary to interact with native APIs like with SQLite,
// and that can live in its own separate driver crate.
#![forbid(unsafe_code)]
// Allows an API be documented as only available in some specific platforms.
// <https://doc.rust-lang.org/unstable-book/language-features/doc-cfg.html>
#![cfg_attr(docsrs, feature(doc_cfg))]

#[macro_use]
pub mod ext;

#[macro_use]
pub mod error;

#[macro_use]
pub mod arguments;

#[macro_use]
pub mod pool;

pub mod connection;

#[macro_use]
pub mod transaction;

#[macro_use]
pub mod encode;

#[macro_use]
pub mod decode;

#[macro_use]
pub mod types;

#[macro_use]
pub mod query;

#[macro_use]
pub mod acquire;

#[macro_use]
pub mod column;

#[macro_use]
pub mod statement;

pub mod common;
pub mod database;
pub mod describe;
pub mod executor;
pub mod from_row;
pub mod fs;
pub mod io;
pub mod logger;
pub mod net;
pub mod query_as;
pub mod query_builder;
pub mod query_scalar;
pub mod sql_str;

pub mod raw_sql;
pub mod row;
pub mod rt;
pub mod sync;
pub mod type_checking;
pub mod type_info;
pub mod value;

#[cfg(feature = "migrate")]
pub mod migrate;

#[cfg(feature = "any")]
pub mod any;

// Implements test support with automatic DB management.
#[cfg(feature = "migrate")]
pub mod testing;

pub mod config;

pub use bytes;
pub use either::Either;
pub use error::Error;
pub use error::Result;
pub use hashbrown::HashMap;
pub use hashbrown::hash_map;
pub use indexmap::IndexMap;
pub use percent_encoding;
pub use smallvec::SmallVec;
pub use url::Url;
pub use url::{self};

/// Helper module to get drivers compiling again that used to be in this crate,
/// to avoid having to replace tons of `use crate::<...>` imports.
///
/// This module can be glob-imported and should not clash with any modules a driver
/// would want to implement itself.
pub mod driver_prelude {
    pub use either::Either;

    pub use crate::HashMap;
    pub use crate::acquire;
    pub use crate::common;
    pub use crate::decode;
    pub use crate::describe;
    pub use crate::encode;
    pub use crate::error::Error;
    pub use crate::error::Result;
    pub use crate::executor;
    pub use crate::ext;
    pub use crate::from_row;
    pub use crate::fs;
    pub use crate::hash_map;
    pub use crate::io;
    pub use crate::logger;
    pub use crate::net;
    pub use crate::pool;
    pub use crate::query;
    pub use crate::query_as;
    pub use crate::query_builder;
    pub use crate::query_scalar;
    pub use crate::rt;
    pub use crate::sync;
}
