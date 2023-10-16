#[allow(unused_imports)]
use std::sync::mpsc;
use std::{
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant},
};

use anyhow::{bail, Result};
use chrono::prelude::*;

mod kvm;
use kstat::consts::*;
mod kstat;
mod term;
use term::Line;

enum Activity {
    Line(term::Line),
    Error(String),
}

struct FillMem {
    inner: Arc<(Mutex<Inner>, Condvar)>,
}

impl FillMem {
    fn unbusy(&self) {
        self.inner.0.lock().unwrap().busy = false;
        self.inner.1.notify_all();
    }
}

struct Inner {
    busy: bool,
    interrupt: bool,
}

fn main() -> Result<()> {
    //let kvm = kvm::Kvm::new()?;
    let mut ks = kstat::KstatWrapper::open()?;
    let ed0 = Arc::new(term::Term::start()?);
    let fm0 = Arc::new(FillMem {
        inner: Arc::new((
            Mutex::new(Inner { busy: false, interrupt: false }),
            Condvar::new(),
        )),
    });

    let (tx0, rx) = mpsc::channel();

    let tx = tx0.clone();
    let ed = Arc::clone(&ed0);
    std::thread::Builder::new()
        .name("timer".into())
        .spawn(move || {
            let interval = Duration::from_millis(500);
            let mut last_run = Instant::now();

            loop {
                std::thread::sleep(interval);

                /*
                 * Warn the user if we appear to be sluggish.
                 */
                let now = Instant::now();
                let msec =
                    now.checked_duration_since(last_run).unwrap().as_millis();
                if msec > 3 * interval.as_millis() {
                    ed.log(&format!("{msec} msec since last stats; sluggish?"))
                        .ok();
                }
                last_run = now;

                let mut arc_c: u64 = 0;
                let mut arc_c_min: u64 = 0;
                let mut arc_c_max: u64 = 0;
                let mut availrmem: u64 = 0;
                let mut freemem: u64 = 0;

                /*
                 * Read some statistics from the kernel to emit.
                 */
                let now = Utc::now();
                if ks.chain_update().is_err() {
                    continue;
                }

                ks.lookup(Some(MODULE_ZFS), Some(NAME_ARCSTATS));
                while ks.step() {
                    if ks.module() != MODULE_ZFS || ks.name() != NAME_ARCSTATS {
                        break;
                    }

                    arc_c = ks.data_u64(STAT_C).unwrap_or(0);
                    arc_c_min = ks.data_u64(STAT_C_MIN).unwrap_or(0);
                    arc_c_max = ks.data_u64(STAT_C_MAX).unwrap_or(0);
                    break;
                }

                ks.lookup(Some(MODULE_UNIX), Some(NAME_SYSTEM_PAGES));
                while ks.step() {
                    if ks.module() != MODULE_UNIX
                        || ks.name() != NAME_SYSTEM_PAGES
                    {
                        break;
                    }

                    availrmem = ks.data_u64(STAT_AVAILRMEM).unwrap_or(0);
                    freemem = ks.data_u64(STAT_FREEMEM).unwrap_or(0);
                    break;
                }

                let mut out = now.format("%H:%M:%S%.3fZ").to_string();
                for (n, v, p) in [
                    ("c", arc_c, false),
                    ("min", arc_c_min, false),
                    ("max", arc_c_max, false),
                    ("free", freemem, true),
                    ("avrm", availrmem, true),
                ] {
                    let v =
                        if p { v * 4096 } else { v } as f64 / 1024.0 / 1024.0;
                    out.push_str(&format!(" {n} {v:7.1}"));
                }

                if ed.log(&format!("{out}")).is_err() {
                    return;
                }
            }
        })
        .unwrap();

    let tx = tx0.clone();
    let ed = Arc::clone(&ed0);
    let fm = Arc::clone(&fm0);
    std::thread::Builder::new()
        .name("editor".into())
        .spawn(move || loop {
            {
                let mut i = fm.inner.0.lock().unwrap();
                while i.busy {
                    i = fm.inner.1.wait(i).unwrap();
                }
            }

            match ed.line() {
                Ok(l) => {
                    {
                        let mut i = fm.inner.0.lock().unwrap();
                        i.busy = true;
                    }

                    if tx.send(Activity::Line(l)).is_err() {
                        return;
                    }
                }
                Err(e) => {
                    tx.send(Activity::Error(e.to_string())).ok();
                    return;
                }
            }
        })
        .unwrap();

    let mut allocs: Vec<Vec<u8>> = Vec::new();

    let ed = ed0;
    let fm = fm0;
    'cmd: loop {
        match rx.recv().unwrap() {
            Activity::Line(Line::Line(l)) => {
                let t = l.split_whitespace().collect::<Vec<_>>();

                match t.get(0) {
                    Some(&"touch") => {
                        let start = Instant::now();
                        let mut sz: u64 = 0;
                        let mut c: u64 = 0;
                        for a in allocs.iter_mut() {
                            for i in 0..a.len() {
                                a[i] += 1;
                                sz += 1;

                                c += 1;
                                if c % 10000 == 0 {
                                    if ed.take_ctrlc() {
                                        ed.log("interrupted!")?;
                                        fm.unbusy();
                                        continue 'cmd;
                                    }
                                }
                            }
                        }

                        let dur = Instant::now()
                            .checked_duration_since(start)
                            .unwrap();
                        let mb = sz / 1024 / 1024;
                        ed.log(&format!(
                            "touched {mb} megabytes in {} msec",
                            dur.as_millis()
                        ))?;
                        fm.unbusy();
                    }
                    Some(&"grow") => match t.get(1) {
                        Some(megs) => match megs.parse::<usize>() {
                            Ok(megs) => {
                                fm.inner.0.lock().unwrap().busy = true;
                                fm.inner.1.notify_all();

                                let start = Instant::now();
                                let sz = megs * 1024 * 1024;
                                let mut c: u64 = 0;
                                let mut a = Vec::with_capacity(sz);
                                while a.len() < sz {
                                    a.push(b'A');

                                    c += 1;
                                    if c % 10000 == 0 {
                                        if ed.take_ctrlc() {
                                            ed.log("interrupted!")?;
                                            fm.unbusy();
                                            continue 'cmd;
                                        }
                                    }
                                }
                                allocs.push(a);

                                let dur = Instant::now()
                                    .checked_duration_since(start)
                                    .unwrap();
                                ed.log(&format!(
                                    "grew by {megs} megabytes in {} msec",
                                    dur.as_millis()
                                ))?;
                                fm.unbusy();
                            }
                            Err(e) => {
                                ed.log(&e.to_string())?;
                            }
                        },
                        None => ed.log("grow by how much?")?,
                    },
                    Some(other) => {
                        ed.log(&format!("{other:?} not understood"))?;
                        fm.unbusy();
                    }
                    None => {
                        fm.unbusy();
                    }
                }
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
