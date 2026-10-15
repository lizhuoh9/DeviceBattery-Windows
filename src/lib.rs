//! Device Battery for Windows: a desktop widget and tray icon that show the battery of
//! wireless mice, keyboards and headphones, including 2.4 GHz receivers Windows cannot read.

pub mod common;

#[cfg(windows)]
pub mod win;
