use std::{
    collections::VecDeque,
    io::{Read, Write},
    mem::MaybeUninit,
    os::fd::{AsRawFd, RawFd},
    sync::{atomic::AtomicBool, mpsc, Arc, Condvar, Mutex},
    time::Duration,
};

use anyhow::{bail, Result};
use libc::{TCIOFLUSH, TCSADRAIN, TCSANOW};
use termios::Termios;

pub struct Term {
    orig_termios: Termios,
    cleaned_up: bool,

    output: Mutex<Box<dyn ForOut>>,
    sigterm: Arc<AtomicBool>,

    inner: Arc<(Mutex<Inner>, Condvar)>,
}

enum Event {
    Input(u8),
    Interrupted,
    Log(String),
    End,
}

struct Inner {
    state: State,
    input: VecDeque<u8>,
    eof: bool,
    interrupted: bool,
    ctrlc: bool,
    buffer: String,
    prompt: String,
    sigterm_delivered: bool,
    log: Option<String>,
}

enum State {
    Rest,
    Editing,
    CleanedUp,
}

pub enum Line {
    Line(String),
    End,
}

trait ForIn: Read + AsRawFd + Send {}
trait ForOut: Write + AsRawFd + Send {}

impl ForIn for std::fs::File {}
impl ForOut for std::fs::File {}

impl ForIn for std::io::Stdin {}
impl ForOut for std::io::Stdout {}

pub struct WinSize {
    height: usize,
    width: usize,
}

pub fn getwinsz(fd: RawFd) -> std::io::Result<WinSize> {
    let mut winsize: MaybeUninit<libc::winsize> = MaybeUninit::uninit();
    let r = unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, winsize.as_mut_ptr()) };
    if r != 0 {
        Err(std::io::Error::last_os_error())
    } else {
        let winsize = unsafe { winsize.assume_init() };
        Ok(WinSize {
            height: winsize.ws_row as usize,
            width: winsize.ws_col as usize,
        })
    }
}

impl Term {
    pub fn start() -> Result<Term> {
        /*
         * Register for SIGTERM so that we can draw a final screen.
         */
        let sigterm = Arc::new(AtomicBool::new(false));
        signal_hook::flag::register(
            signal_hook::consts::SIGTERM,
            Arc::clone(&sigterm),
        )
        .unwrap();

        let (input, output): (Box<dyn ForIn>, Box<dyn ForOut>) =
            (Box::new(std::io::stdin()), Box::new(std::io::stdout()));

        let _sz = getwinsz(output.as_raw_fd())?;

        let mut inner0 = Arc::new((
            Mutex::new(Inner {
                state: State::Rest,
                buffer: "".into(),
                prompt: "fillmem> ".into(),
                sigterm_delivered: false,
                eof: false,
                interrupted: false,
                input: Default::default(),
                log: None,
                ctrlc: false,
            }),
            Condvar::new(),
        ));

        /*
         * Create a thread to process input from stdin.
         */
        let inner = Arc::clone(&inner0);
        std::thread::Builder::new()
            .name("stdin".into())
            .spawn(move || {
                let mut br = std::io::BufReader::new(input);
                let mut buf = [0u8; 1];

                loop {
                    match br.read(&mut buf) {
                        Ok(0) => {
                            let mut i = inner.0.lock().unwrap();
                            i.eof = true;
                            inner.1.notify_all();
                            return;
                        }
                        Ok(1) => {
                            let mut i = inner.0.lock().unwrap();

                            if buf[0] == 0x03 {
                                /*
                                 * Handle ^C interrupt by dropping the inbound
                                 * buffer and reporting immediately.
                                 */
                                i.ctrlc = true;
                                i.input.clear();
                            } else {
                                i.input.push_back(buf[0]);
                            }

                            inner.1.notify_all();
                        }
                        Ok(n) => {
                            panic!("{} is not the correct number of bytes", n)
                        }
                        Err(e)
                            if e.kind() == std::io::ErrorKind::Interrupted =>
                        {
                            let mut i = inner.0.lock().unwrap();
                            i.interrupted = true;
                            inner.1.notify_all();
                        }
                        _ => return, /* XXX */
                    };
                }
            })
            .unwrap();

        /*
         * Put the terminal in raw mode.
         */
        let orig_termios = termios::Termios::from_fd(output.as_raw_fd())?;
        let mut termios = orig_termios.clone();
        termios::cfmakeraw(&mut termios);
        termios::tcsetattr(output.as_raw_fd(), TCSANOW, &termios)?;
        termios::tcflush(output.as_raw_fd(), TCIOFLUSH)?;

        Ok(Term {
            orig_termios,
            cleaned_up: false,
            output: Mutex::new(output),
            sigterm,
            inner: inner0,
        })
    }

    pub fn log(&self, msg: &str) -> Result<()> {
        let mut i = self.inner.0.lock().unwrap();

        loop {
            match i.state {
                State::Rest => {
                    self.emit(msg)?;
                    self.emit("\r\n")?;
                    return Ok(());
                }
                State::CleanedUp => {
                    println!("{msg}");
                    return Ok(());
                }
                State::Editing => {
                    if i.log.is_some() {
                        i = self.inner.1.wait(i).unwrap();
                        continue;
                    }

                    i.log = Some(msg.to_string());
                    self.inner.1.notify_all();
                    return Ok(());
                }
            }
        }
    }

    fn emit(&self, msg: &str) -> Result<()> {
        let mut out = self.output.lock().unwrap();

        out.write_all(msg.as_bytes())?;
        out.flush()?;
        Ok(())
    }

    fn redraw_prompt(&self, i: &Inner) -> Result<()> {
        self.emit("\r\x1b[0K")?;
        self.emit(&i.prompt)?;
        self.emit(&i.buffer)?;
        Ok(())
    }

    fn log_while_editing(&self, i: &Inner, msg: &str) -> Result<()> {
        /*
         * To print a log line while we are editing we need to move
         * the cursor back to the front of the line, clear
         * everything to our right, emit the line, then draw the
         * prompt and buffer contents right after.
         */
        self.emit("\r\x1b[0K")?;
        self.emit(msg)?;
        self.emit("\r\n")?;
        self.redraw_prompt(i)
    }

    pub fn take_ctrlc(&self) -> bool {
        let mut i = self.inner.0.lock().unwrap();
        if !i.ctrlc {
            return false;
        }

        i.ctrlc = false;

        drop(i);
        self.log("^C").ok();

        true
    }

    pub fn line(&self) -> Result<Line> {
        let mut i = self.inner.0.lock().unwrap();

        match i.state {
            State::Rest => {
                i.buffer.clear();
                i.state = State::Editing;
                self.inner.1.notify_all();
            }
            State::Editing => bail!("another thread is already editing"),
            State::CleanedUp => bail!("cleaned up already"),
        }

        self.emit(&i.prompt)?;

        /*
         * Listen for input and process it.
         */
        let timeo = Duration::from_millis(500);
        loop {
            if self.sigterm.load(std::sync::atomic::Ordering::Relaxed) {
                /*
                 * Begin tearing down.
                 */
                drop(i);
                self.cleanup();
                /*
                 * XXX
                 */
                return Ok(Line::End);
            }

            if i.ctrlc {
                /*
                 * XXX
                 */
                i.ctrlc = false;
                drop(i);
                self.cleanup();
                return Ok(Line::End);
            }

            if i.interrupted {
                i.interrupted = false;
                continue;
            }

            if i.eof {
                drop(i);
                self.cleanup();
                return Ok(Line::End);
            }

            if let Some(msg) = i.log.take() {
                self.inner.1.notify_all();
                self.log_while_editing(&i, &msg)?;
                continue;
            }

            let b = if let Some(b) = i.input.pop_front() {
                self.inner.1.notify_all();
                b
            } else {
                i = self.inner.1.wait_timeout_ms(i, 250).unwrap().0;
                continue;
            };

            if b.is_ascii_graphic() || b == b' ' {
                if i.buffer.len() < 60 {
                    /*
                     * XXX this will do for now
                     */
                    i.buffer.push(b as char);
                    self.emit(&format!("{}", b as char))?;
                }
            } else if b == 0x03 {
                /*
                 * XXX ^C
                 */
                drop(i);
                self.cleanup();
                return Ok(Line::End);
            } else if b == 0x04 {
                /*
                 * XXX ^D
                 */
                drop(i);
                self.cleanup();
                return Ok(Line::End);
            } else if b == 0x0d {
                /*
                 * XXX CR
                 */
                i.state = State::Rest;
                self.inner.1.notify_all();
                let buf = i.buffer.clone();
                i.buffer.clear();
                self.emit("\r\n")?;
                return Ok(Line::Line(buf));
            } else if b == 0x7f {
                if !i.buffer.is_empty() {
                    let nl = i.buffer.len() - 1;
                    i.buffer.truncate(nl);
                    self.emit("\x08 \x08")?; /* XXX */
                }
            } else if b == 0x15 {
                /*
                 * XXX ^U
                 */
                i.buffer.clear();
                self.redraw_prompt(&i)?;
            } else {
                self.log_while_editing(&i, &format!("unknown b: {b:?}"))?;

                /*
                 * Ring the bell!
                 */
                self.emit("\x07")?;
            }
        }
    }

    pub fn cleanup(&self) {
        let mut i = self.inner.0.lock().unwrap();

        if matches!(i.state, State::CleanedUp) {
            return;
        }

        /*
         * Clean up the terminal and restore the original termios attributes:
         */
        let mut output = self.output.lock().unwrap();
        output.write_all(b"\r\n").ok();
        output.flush().ok();
        termios::tcsetattr(output.as_raw_fd(), TCSADRAIN, &self.orig_termios)
            .ok();

        i.state = State::CleanedUp;
        self.inner.1.notify_all();
    }
}

impl Drop for Term {
    fn drop(&mut self) {
        self.cleanup();
    }
}
