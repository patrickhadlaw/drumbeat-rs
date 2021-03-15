use super::observable::{DummyOwner, Owner, Signal};
use super::scheduler::Scheduler;
use crate::sync::threadpool::Task;

use std::sync::{Arc, Weak};

pub struct Subscription {
  subscriber: Weak<dyn Owner>,
  _scheduler: Option<Arc<dyn Scheduler>>,
}

impl Subscription {
  pub(super) fn new(subscriber: Weak<dyn Owner>) -> Self {
    Subscription {
      subscriber: subscriber.clone(),
      _scheduler: subscriber.upgrade().map(|subscriber| subscriber.scheduler()),
    }
  }

  pub fn unsubscribe(&mut self) {
    if let Some(subscriber) = self.subscriber.upgrade() {
      self.subscriber = Weak::<DummyOwner>::new();
      if let Some(owner) = subscriber.owner() {
        owner.handle(Signal::Recycle(subscriber.id()))
      }
    }
  }

  pub fn active(&self) -> bool {
    self.subscriber.upgrade().is_some()
  }

  pub fn finalize<F>(&mut self, task: F) -> Self
  where
    F: Fn() + Send + Sync + 'static,
  {
    match self.subscriber.upgrade() {
      Some(owner) => {
        self.subscriber = Weak::<DummyOwner>::new();
        owner.add_finalize(Task::new(task));
        Subscription::new(Arc::downgrade(&owner))
      }
      None => Subscription::new(Weak::<DummyOwner>::new()),
    }
  }
}

impl Drop for Subscription {
  fn drop(&mut self) {
    self.unsubscribe();
  }
}
