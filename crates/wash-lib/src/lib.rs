//! A crate that implements the functionality behind the wasmCloud shell
//!
//! The `wash` command line interface <https://github.com/wasmcloud/wash> is a great place
//! to find examples on how to fully utilize this library.

#[cfg(feature = "start")]
/// Functionality related to downloading and starting wasmCloud components, including a NATS server binary and
/// the wasmCloud host tarball.
pub mod start;
