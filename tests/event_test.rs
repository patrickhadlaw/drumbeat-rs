use drumbeat::event::observable::Observable;
use drumbeat::event::ops::*;
use drumbeat::event::scheduler::SchedulerType;
use drumbeat::event::subject::{BasicSubject, BasicSubjectBuilder, Subject};
use drumbeat::utils::testing;

use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Mutex};

#[test]
fn simple_event_test() {
  println!("START simple_event_test");
  testing::async_context(|| {
    let basic = BasicSubject::new();
    let finalize = Arc::new(AtomicBool::new(false));
    let (tx, rx) = std::sync::mpsc::channel();
    let tx = Mutex::new(tx.clone());
    {
      let cloned = finalize.clone();
      let _subscription = basic
        .observe()
        .pipe()
        .subscribe(move |x| {
          tx.lock().unwrap().send(x).unwrap();
        })
        .finalize(move || {
          cloned.store(true, Ordering::Relaxed);
        });
      basic.next("test".to_owned());
      assert_eq!(rx.recv().unwrap(), "test");
      assert_eq!(finalize.load(Ordering::Relaxed), false);
    }
    assert_eq!(finalize.load(Ordering::Relaxed), true);
  });
  println!("END simple_event_test");
}

#[test]
fn take_observable_of_test() {
  println!("START take_observable_of_test");
  testing::async_context(|| {
    let sum = Arc::new(AtomicI32::new(0));
    let subscribe_sum = sum.clone();
    let finalize_sum = sum.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let tx = Mutex::new(tx.clone());
    let observable = Observable::of(vec![1, 2, 3]);
    {
      let _subscription = observable
        .pipe()
        .take(3)
        .subscribe(move |x| {
          subscribe_sum.fetch_add(x, Ordering::Relaxed);
        })
        .finalize(move || {
          tx.lock()
            .unwrap()
            .send(finalize_sum.load(Ordering::Relaxed))
            .unwrap();
        });
      assert_eq!(rx.recv().unwrap(), 6);
    }
  });
  println!("END take_observable_of_test");
}

#[test]
fn first_subject_test() {
  println!("START first_subject_test");
  testing::async_context(|| {
    let sum = Arc::new(AtomicI32::new(0));
    let subscribe_sum = sum.clone();
    let finalize_sum = sum.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let tx = Mutex::new(tx.clone());
    let subject = BasicSubjectBuilder::new()
      .scheduler(SchedulerType::Blocking)
      .build();
    {
      let _subscription = subject
        .observe()
        .pipe()
        .first()
        .subscribe(move |x| {
          subscribe_sum.fetch_add(x, Ordering::Relaxed);
        })
        .finalize(move || {
          tx.lock()
            .unwrap()
            .send(finalize_sum.load(Ordering::Relaxed))
            .unwrap();
        });
      subject.next(5)
    }
    assert_eq!(rx.recv().unwrap(), 5);
  });
  println!("END first_subject_test");
}

#[test]
fn map_event_to_string_test() {
  println!("START map_event_to_string_test");
  testing::async_context(|| {
    let subject = BasicSubject::new();
    let rx = subject
      .observe()
      .pipe()
      .take(3)
      .map(|x| format!("{}_", x))
      .collect();
    subject.next("abc");
    subject.next("def");
    subject.next("ghi");
    let mut result = rx.recv().unwrap();
    result.sort();
    assert_eq!(result, ["abc_", "def_", "ghi_"]);
  });
  println!("END map_event_to_string_test");
}

#[test]
fn complex_pipe_test() {
  println!("START complex_pipe_test");
  testing::async_context(|| {
    let subject = BasicSubjectBuilder::new()
      .scheduler(SchedulerType::Worker)
      .build::<String>();
    let rx = subject
      .observe()
      .pipe()
      .take(3)
      .filter(|x| x.len() > 1)
      .map(|x| format!("{}_", x))
      .skip(1)
      .collect();
    subject.next("A".to_owned());
    subject.next("BBB".to_owned());
    subject.next("CCC".to_owned());
    assert_eq!(rx.recv().unwrap(), ["CCC_"]);
  });
  println!("END complex_pipe_test");
}

#[test]
fn multi_observable_test() {
  println!("START multi_observable_test");
  testing::async_context(|| {
    let subject = BasicSubjectBuilder::new()
      .scheduler(SchedulerType::Worker)
      .build();
    let done = BasicSubjectBuilder::new()
      .scheduler(SchedulerType::Blocking)
      .build();
    let list = Arc::new(Mutex::new(Vec::new()));
    let cloned = list.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let tx = Mutex::new(tx);
    let done_clone = done.clone();
    let _subscription = subject
      .observe()
      .pipe()
      .take_until(done.observe())
      .tap(move |x| {
        if x == "C" {
          done_clone.next(());
        }
      })
      .map(|x| format!("{}+", x))
      .subscribe(move |x| {
        cloned.lock().unwrap().push(x);
      })
      .finalize(move || {
        tx.lock().unwrap().send(()).unwrap();
      });
    subject.next("A".to_owned());
    subject.next("B".to_owned());
    subject.next("C".to_owned());
    subject.next("D".to_owned());
    subject.next("E".to_owned());
    subject.next("F".to_owned());
    rx.recv().unwrap();
    list.lock().unwrap().sort();
    assert_eq!(*list.lock().unwrap(), ["A+", "B+", "C+"]);
  });
  println!("END multi_observable_test");
}
