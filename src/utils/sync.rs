#[cfg(not(test))]
pub fn kill_process_on_panic() {
  let original_hook = std::panic::take_hook();
  std::panic::set_hook(Box::new(move |panic_info| {
    original_hook(panic_info);
    let current = std::thread::current();
    let name = if let Some(name) = current.name() {
      name
    } else {
      "unknown"
    };
    if !log::log_enabled!(log::Level::Error) {
      println!(
        "unhandled panic on thread '{}' with info: '{}'",
        name, panic_info
      );
    }
    log::error!(
      "unhandled panic on thread '{}' with info: '{}'",
      name,
      panic_info
    );
    std::process::exit(-1);
  }));
}

#[cfg(test)]
pub fn kill_process_on_panic() {}
