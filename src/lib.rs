//! Drumbeat is:
//! * a real-time focused, observer pattern based, multithreaded event system.
//! * a real-time focused synchronization library implementing thread workers,
//!   thread pools and a multithreaded runtime.
#[macro_use]
extern crate lazy_static;

pub mod event;
pub mod sync;
pub mod utils;
