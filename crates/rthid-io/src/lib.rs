//! The filesystem side of rthid, with everything that reads or writes files,
//! so the binary's App and dialogs stay IO free.

pub mod atomic;
pub mod dir;
pub mod image;
pub mod io;
pub mod paste;
pub mod path;
pub mod png;
