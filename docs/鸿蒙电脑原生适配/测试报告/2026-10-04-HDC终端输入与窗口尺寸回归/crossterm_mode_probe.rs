use std::io::{self, IsTerminal};

fn main() {
    println!("backend={} stdin_isatty={} stdout_isatty={}",
        if cfg!(feature = "libc-backend") { "libc" } else { "rustix" },
        io::stdin().is_terminal(), io::stdout().is_terminal());
    let mut passed = true;
    for cycle in 1..=3 {
        match crossterm::terminal::enable_raw_mode() {
            Ok(()) => {
                let active = crossterm::terminal::is_raw_mode_enabled().unwrap_or(false);
                println!("cycle={cycle} stage=enable_raw_mode result=ok active={active}");
                match crossterm::terminal::disable_raw_mode() {
                    Ok(()) => println!("cycle={cycle} stage=disable_raw_mode result=ok"),
                    Err(err) => { println!("cycle={cycle} stage=disable_raw_mode errno={:?}", err.raw_os_error()); passed = false; break; }
                }
            }
            Err(err) => { println!("cycle={cycle} stage=enable_raw_mode errno={:?}", err.raw_os_error()); passed = false; break; }
        }
    }
    println!("terminal_probe_complete passed={passed}");
    if !passed { std::process::exit(1); }
}
