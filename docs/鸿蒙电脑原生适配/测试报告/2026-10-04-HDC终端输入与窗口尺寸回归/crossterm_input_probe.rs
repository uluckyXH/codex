use std::io::{self, IsTerminal};
use std::time::Duration;

#[path = "../../../../codex-rs/tui/src/terminal_size.rs"]
mod terminal_size;

fn main() -> io::Result<()> {
    println!("backend=rustix stdin_isatty={} stdout_isatty={}",
        io::stdin().is_terminal(), io::stdout().is_terminal());
    let native = crossterm::terminal::size()?;
    let resolved = terminal_size::resolve(native);
    println!("native_columns={} native_rows={} resolved_columns={} resolved_rows={}",
        native.0, native.1, resolved.0, resolved.1);
    let expected = if std::env::args().any(|arg| arg == "--expect-hint") {
        (120, 36)
    } else {
        (80, 24)
    };
    if native != (0, 0) || resolved != expected {
        return Err(io::Error::other("unexpected terminal geometry"));
    }
    let mut original: libc::termios = unsafe { std::mem::zeroed() };
    if unsafe { libc::tcgetattr(0, &mut original) } != 0 {
        return Err(io::Error::last_os_error());
    }
    crossterm::terminal::enable_raw_mode()?;
    println!("__TERMINAL_INPUT_READY__");
    let input = (|| {
        if !crossterm::event::poll(Duration::from_secs(5))? {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "no terminal input"));
        }
        let event = crossterm::event::read()?;
        println!("input_event={event:?}");
        if matches!(event, crossterm::event::Event::Key(key) if key.code == crossterm::event::KeyCode::Char('K')) {
            Ok(())
        } else {
            Err(io::Error::other("unexpected terminal input"))
        }
    })();
    crossterm::terminal::disable_raw_mode()?;
    let mut after: libc::termios = unsafe { std::mem::zeroed() };
    if unsafe { libc::tcgetattr(0, &mut after) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let restored = original.c_iflag == after.c_iflag && original.c_oflag == after.c_oflag &&
        original.c_cflag == after.c_cflag && original.c_lflag == after.c_lflag && original.c_cc == after.c_cc;
    println!("modes_restored={restored} input_passed={}", input.is_ok());
    input?;
    if !restored { return Err(io::Error::other("terminal modes not restored")); }
    println!("crossterm_input_probe_complete passed=true");
    Ok(())
}
