//! Platform-independent logic: receiver protocols, the reading store, text and alerts.
//! Everything here is unit-tested on any host (`cargo test`).

pub mod alerts;
pub mod asus;
pub mod bluetooth;
pub mod channel;
pub mod compx;
pub mod hid_path;
pub mod hidpp;
pub mod logitech;
pub mod model;
pub mod presentation;
pub mod store;
pub mod strings;
