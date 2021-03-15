//! Drumbeat synchronization mechanisms.
//!
//! ## Why reinvent the wheel?
//!
//! This crate's goal is to be aimed towards real-time applications such as GUIs
//! and game engine's, for this reason the synchroniztion mechanisms are going
//! to be custom built with this goal in mind. For these applications the order
//! of execution is important so there should be a choice in how things are
//! scheduled.
pub mod buffer;
pub mod executor;
pub mod runtime;
pub mod spinlock;
pub mod threadpool;
pub(crate) mod task;
pub mod worker;
