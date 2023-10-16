use std::sync::Arc;
#[allow(unused_imports)]
use std::sync::mpsc;

use anyhow::{bail, Result};

mod term;
use term::Line;

enum Activity {
    Timer,
    Line(term::Line),
    Error(String),
}

fn main() -> Result<()> {
    let ed0 = Arc::new(term::Term::start()?);

    let (tx0, rx) = mpsc::channel();

    let tx = tx0.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
        if tx.send(Activity::Timer).is_err() {
            return;
        }
    });

    let tx = tx0.clone();
    let ed = Arc::clone(&ed0);
    std::thread::spawn(move || loop {
        match ed.line() {
            Ok(l) => {
                if tx.send(Activity::Line(l)).is_err() {
                    return;
                }
            }
            Err(e) => {
                tx.send(Activity::Error(e.to_string())).ok();
                return;
            }
        }
    });

    let ed = ed0;
    loop {
        match rx.recv().unwrap() {
            Activity::Timer => {
                ed.log("timer!")?;
            }
            Activity::Line(Line::Line(l)) => {
                ed.log(&format!(" * input line: {l:?}"))?;
            }
            Activity::Line(Line::End) => {
                ed.log(" * end!")?;
                break;
            }
            Activity::Error(e) => {
                ed.cleanup();
                bail!(e);
            }
        }
    }

    ed.cleanup();
    Ok(())
}
