//! Minimal direction type for the shared log parser.
//!
//! The CLI's `frame::Dir` carries output-formatting helpers; the GUI only
//! needs the direction values, so this keeps the log viewer independent of the
//! command-line writer stack.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Rx,
    Tx,
    Error,
}
