use super::observable::{DummyOwner, Observable, ObservableType, Owner, Signal};
use crate::sync::buffer::RingBuffer;
use crate::sync::threadpool::Task;

use std::collections::VecDeque;
use std::slice::Iter;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

type InvokerFn<T> = dyn Fn(T) -> Signal + Send + Sync;

#[derive(Clone)]
pub struct Invoker<T>
where
  T: ObservableType,
{
  func: Arc<InvokerFn<T>>,
}

impl<T> Invoker<T>
where
  T: ObservableType,
{
  pub(super) fn new(func: Arc<InvokerFn<T>>) -> Self {
    Invoker { func }
  }

  pub(super) fn identity(observable: Arc<Observable<T>>) -> Self {
    Invoker {
      func: Arc::new(move |x| {
        observable.next(x);
        Signal::None
      }),
    }
  }

  pub(super) fn invoke(&self, input: T) -> Signal {
    (self.func)(input)
  }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DispatcherType {
  Basic,
  Replay(usize),
}

pub trait Replicable {
  fn get_type(&self) -> DispatcherType;
}

pub(super) fn create<T>(dispatch_type: DispatcherType) -> Box<dyn Dispatcher<T>>
where
  T: ObservableType,
{
  match dispatch_type {
    DispatcherType::Basic => Box::new(BasicDispatcher::new()),
    DispatcherType::Replay(size) => Box::new(ReplayDispatcher::new(size, false)),
  }
}

pub(super) fn replicate<T>(replicate: &dyn Replicable) -> Box<dyn Dispatcher<T>>
where
  T: ObservableType,
{
  match replicate.get_type() {
    DispatcherType::Basic => Box::new(BasicDispatcher::new()),
    DispatcherType::Replay(size) => Box::new(ReplayDispatcher::new(size, true)),
  }
}

pub(super) trait Dispatcher<T>: Replicable + Send + Sync
where
  T: ObservableType,
{
  fn initialize(&self);
  fn children(&self) -> Iter<'_, DispatchTarget<T>>;
  fn num_children(&self) -> usize;
  fn add_child(&mut self, target: DispatchTarget<T>);
  fn bootstrap(&self, child: usize) -> Option<Task>;
  fn remove_child(&mut self, id: usize) -> Option<Arc<dyn Owner>>;
  fn owner(&self) -> Option<Arc<dyn Owner>>;
  fn set_owner(&mut self, owner: Arc<dyn Owner>);
  fn dispatch(&self, value: T) -> Option<Vec<Signal>>;
  fn finalize(&self);
  fn add_finalize(&self, task: Task);
  fn as_replicable(&self) -> &dyn Replicable;
}

#[derive(Clone)]
pub(super) struct DispatchTarget<T>(pub(super) Arc<dyn Owner>, pub(super) Invoker<T>)
where
  T: ObservableType;

impl<T> DispatchTarget<T>
where
  T: ObservableType,
{
  pub fn new(owner: Arc<dyn Owner>, invoker: Invoker<T>) -> Self {
    DispatchTarget(owner, invoker)
  }
}

pub(super) struct BasicDispatcher<T>
where
  T: ObservableType,
{
  children: Vec<DispatchTarget<T>>,
  owner: Weak<dyn Owner>,
  finalize: Mutex<VecDeque<Task>>,
}

impl<T> BasicDispatcher<T>
where
  T: ObservableType,
{
  pub fn new() -> Self {
    BasicDispatcher {
      children: Vec::new(),
      owner: Weak::<DummyOwner>::new(),
      finalize: Mutex::new(VecDeque::new()),
    }
  }
}

impl<T> Dispatcher<T> for BasicDispatcher<T>
where
  T: ObservableType,
{
  fn initialize(&self) {}

  fn children(&self) -> Iter<'_, DispatchTarget<T>> {
    self.children.iter()
  }

  fn num_children(&self) -> usize {
    self.children.len()
  }

  fn add_child(&mut self, target: DispatchTarget<T>) {
    self.children.push(target);
  }

  fn bootstrap(&self, _child: usize) -> Option<Task> {
    None
  }

  fn remove_child(&mut self, id: usize) -> Option<Arc<dyn Owner>> {
    let idx = self.children.iter().position(|child| child.0.id() == id);
    if let Some(idx) = idx {
      let result = self.children.get(idx).unwrap().0.clone();
      self.children.remove(idx);
      Some(result)
    } else {
      None
    }
  }

  fn owner(&self) -> Option<Arc<dyn Owner>> {
    self.owner.upgrade()
  }

  fn set_owner(&mut self, owner: Arc<dyn Owner>) {
    self.owner = Arc::downgrade(&owner);
  }

  fn dispatch(&self, value: T) -> Option<Vec<Signal>> {
    let mut signals: Option<Vec<Signal>> = None;
    for target in self.children.iter() {
      let DispatchTarget(owner, invoker) = target;
      let signal = invoker.invoke(value.clone());
      if !signal.is_none() {
        if let Some(inner) = &mut signals {
          inner.push(signal);
        } else {
          signals = Some(vec![signal]);
        }
      }
      // Auto-recycle pass
      if owner.finished() {
        match signal {
          Signal::Recycle(_) => (),
          _ => {
            if let Some(inner) = &mut signals {
              inner.push(Signal::Recycle(owner.id()));
            } else {
              signals = Some(vec![Signal::Recycle(owner.id())]);
            }
          }
        }
      }
    }
    signals
  }

  fn finalize(&self) {
    let mut guard = self.finalize.lock().unwrap();
    while let Some(task) = guard.pop_back() {
      task.invoke();
    }
  }

  fn add_finalize(&self, task: Task) {
    self.finalize.lock().unwrap().push_front(task)
  }

  fn as_replicable(&self) -> &dyn Replicable {
    self
  }
}

impl<T> Replicable for BasicDispatcher<T>
where
  T: ObservableType,
{
  fn get_type(&self) -> DispatcherType {
    DispatcherType::Basic
  }
}

pub(super) struct SubscriptionDispatcher<T>
where
  T: ObservableType,
{
  invoker: Invoker<T>,
  owner: Weak<dyn Owner>,
  finalize: Mutex<VecDeque<Task>>,
}

impl<T> SubscriptionDispatcher<T>
where
  T: ObservableType,
{
  pub fn new(invoker: Invoker<T>) -> Self {
    SubscriptionDispatcher {
      invoker,
      owner: Weak::<DummyOwner>::new(),
      finalize: Mutex::new(VecDeque::new()),
    }
  }
}

impl<T> Dispatcher<T> for SubscriptionDispatcher<T>
where
  T: ObservableType,
{
  fn initialize(&self) {}

  fn children(&self) -> Iter<'_, DispatchTarget<T>> {
    [].iter()
  }

  fn num_children(&self) -> usize {
    0
  }

  fn add_child(&mut self, _target: DispatchTarget<T>) {
    unreachable!();
  }

  fn bootstrap(&self, _child: usize) -> Option<Task> {
    None
  }

  fn remove_child(&mut self, _id: usize) -> Option<Arc<dyn Owner>> {
    unreachable!();
  }

  fn owner(&self) -> Option<Arc<dyn Owner>> {
    self.owner.upgrade()
  }

  fn set_owner(&mut self, owner: Arc<dyn Owner>) {
    self.owner = Arc::downgrade(&owner);
  }

  fn dispatch(&self, value: T) -> Option<Vec<Signal>> {
    let signal = self.invoker.invoke(value);
    if !signal.is_none() {
      Some(vec![signal])
    } else {
      None
    }
  }

  fn finalize(&self) {
    let mut guard = self.finalize.lock().unwrap();
    while let Some(task) = guard.pop_back() {
      task.invoke();
    }
  }

  fn add_finalize(&self, task: Task) {
    self.finalize.lock().unwrap().push_front(task)
  }

  fn as_replicable(&self) -> &dyn Replicable {
    self
  }
}

impl<T> Replicable for SubscriptionDispatcher<T>
where
  T: ObservableType,
{
  fn get_type(&self) -> DispatcherType {
    unreachable!();
  }
}

pub(super) struct ReplayDispatcher<T>
where
  T: ObservableType,
{
  base: BasicDispatcher<T>,
  initialized: AtomicBool,
  buffer: RingBuffer<T>,
}

impl<T> ReplayDispatcher<T>
where
  T: ObservableType,
{
  pub fn new(size: usize, replicate: bool) -> Self {
    ReplayDispatcher {
      base: BasicDispatcher::new(),
      initialized: AtomicBool::new(!replicate),
      buffer: RingBuffer::new(size),
    }
  }
}

impl<T> Dispatcher<T> for ReplayDispatcher<T>
where
  T: ObservableType,
{
  fn children(&self) -> Iter<'_, DispatchTarget<T>> {
    self.base.children()
  }

  fn num_children(&self) -> usize {
    self.base.num_children()
  }

  fn add_child(&mut self, target: DispatchTarget<T>) {
    self.base.add_child(target);
  }

  fn initialize(&self) {
    self.initialized.store(true, Ordering::Relaxed);
  }

  fn bootstrap(&self, child: usize) -> Option<Task> {
    if let Some(target) = self.base.children.iter().find(|target| target.0.id() == child) {
      let cloned = target.0.clone();
      let replay = self.buffer.get_and_do(self.buffer.size(), move || cloned.initialize());
      if let Some(owner) = self.owner() {
        let cloned = target.1.clone();
        return Some(Task::new(move || {
          for item in replay.iter() {
            owner.handle(cloned.invoke(item.clone()));
          }
        }));
      }
    }
    None
  }

  fn remove_child(&mut self, id: usize) -> Option<Arc<dyn Owner>> {
    self.base.remove_child(id)
  }

  fn owner(&self) -> Option<Arc<dyn Owner>> {
    self.base.owner()
  }

  fn set_owner(&mut self, owner: Arc<dyn Owner>) {
    self.base.set_owner(owner);
  }

  fn dispatch(&self, value: T) -> Option<Vec<Signal>> {
    if self.initialized.load(Ordering::Relaxed) {
      self.buffer.push(value.clone());
      self.base.dispatch(value)
    } else {
      None
    }
  }

  fn finalize(&self) {
    self.base.finalize()
  }

  fn add_finalize(&self, task: Task) {
    self.base.add_finalize(task)
  }

  fn as_replicable(&self) -> &dyn Replicable {
    self
  }
}

impl<T> Replicable for ReplayDispatcher<T>
where
  T: ObservableType,
{
  fn get_type(&self) -> DispatcherType {
    DispatcherType::Replay(self.buffer.size())
  }
}
