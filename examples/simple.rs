//! A simple example of what drumbeat has to offer

use drumbeat::event::observable::Observable;
use drumbeat::event::ops::*;
use drumbeat::event::subject::{BasicSubject, Subject};

#[derive(Debug, Clone)]
enum EventType {
  EventA(i32),
  EventB(i32),
}

impl EventType {
  fn unwrap(&self) -> i32 {
    match self {
      Self::EventA(value) => *value,
      Self::EventB(value) => *value,
    }
  }
}

impl std::fmt::Display for EventType {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
      Self::EventA(value) => write!(f, "Event A with value '{}'", value)?,
      Self::EventB(value) => write!(f, "Event B with value '{}'", value)?,
    };
    Ok(())
  }
}

fn main() {
  let a = BasicSubject::<EventType>::new();
  let b = BasicSubject::<EventType>::new();
  let merge = Observable::merge(vec![
    a.observe()
      .pipe()
      .map(|x| EventType::EventA(x.unwrap().pow(2)))
      .observe(),
    b.observe().pipe().map(|x| EventType::EventB(x.unwrap() * 3)).observe(),
  ]);
  let rx = merge.pipe().tap(|x| println!("{}", x)).take(6).collect();
  // Since a, b and merge are using the default worker scheduler, the following
  // events will be processed out of order.
  a.next(EventType::EventA(1));
  a.next(EventType::EventA(2));
  a.next(EventType::EventA(3));
  b.next(EventType::EventB(1));
  b.next(EventType::EventB(2));
  b.next(EventType::EventB(3));
  rx.recv().unwrap();
}
