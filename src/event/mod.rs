//! This module contains drumbeat's core event system. The module is organized
//! into the following sub modules:
//! * `dispatcher` which implements the different types of event dispatchers.
//! * `observable` which implements the observable type - a core type which
//!   represents a value in an event sequence.
//! * `ops` which contains all of the observable pipe operators
//! * `subject` which implements all of the observable subject types. These
//!   subjects are used as the source and root of a continuing event stream.
//! * `subscription` which implements the
//!   [Subscription](subscription::Subscription) type which is used to tie a
//!   node in the event stream to the current scope.
//!
pub mod dispatcher;
pub mod observable;
pub mod ops;
pub mod scheduler;
pub mod subject;
pub mod subscription;
