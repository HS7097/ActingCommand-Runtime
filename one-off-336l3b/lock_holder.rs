// One-off (to be reverted), Workflow #336 L3b lock evidence: takes the recording lock through
// the Lab API, reports it, and holds it until a line arrives on stdin (or the process is
// killed). Usage: oneoff_336l3b_lock <state dir> <instance>
use std::io::{BufRead, Write};

fn main() {
    let args = std::env::args().collect::<Vec<_>>();
    let state = std::path::PathBuf::from(&args[1]);
    let lock = match actingcommand_lab::RecordingLock::acquire(&state, &args[2], "oneoff-336l3b holder")
    {
        Ok(lock) => lock,
        Err(error) => {
            println!("FAILED {} {}", error.code, error.message);
            std::process::exit(2);
        }
    };
    println!(
        "LOCKED pid={} path={}",
        std::process::id(),
        lock.lock_path().display()
    );
    std::io::stdout().flush().expect("flush");
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .expect("read stdin");
    drop(lock);
    println!("RELEASED");
}
