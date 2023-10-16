use std::{
    io::{Read, Write},
    mem::MaybeUninit,
    os::fd::{AsRawFd, RawFd},
    sync::{atomic::AtomicBool, mpsc, Arc, Mutex},
    time::Duration,
};

use anyhow::{bail, Result};
use libc::{TCIOFLUSH, TCSADRAIN, TCSANOW};
use termios::Termios;

pub struct Term {
    orig_termios: Termios,
    cleaned_up: bool,

    output: Mutex<Box<dyn ForOut>>,
    tx: mpsc::SyncSender<Event>,
    rx: Mutex<mpsc::Receiver<Event>>,
    sigterm: Arc<AtomicBool>,

    inner: Mutex<Inner>,
}

enum Event {
    Input(u8),
    Interrupted,
    Log(String),
    End,
}

struct Inner {
    state: State,
    buffer: String,
    prompt: String,
    sigterm_delivered: bool,
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

        /*
         * Create a thread to process input from stdin.
         */
        let (tx0, rx) = mpsc::sync_channel(0);
        let tx = tx0.clone();
        std::thread::spawn(move || {
            let mut br = std::io::BufReader::new(input);
            let mut buf = [0u8; 1];

            loop {
                match br.read(&mut buf) {
                    Ok(0) => {
                        tx.send(Event::End).ok();
                        return;
                    }
                    Ok(1) => {
                        if tx.send(Event::Input(buf[0])).is_err() {
                            return;
                        }
                    }
                    Ok(n) => panic!("{} is not the correct number of bytes", n),
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {
                        if tx.send(Event::Interrupted).is_err() {
                            return;
                        }
                    }
                    _ => return, /* XXX */
                };
            }
        });

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
            tx: tx0,
            rx: Mutex::new(rx),
            sigterm,
            inner: Mutex::new(Inner {
                state: State::Rest,
                buffer: "".into(),
                prompt: "fillmem> ".into(),
                sigterm_delivered: false,
            }),
        })
    }

    pub fn log(&self, msg: &str) -> Result<()> {
        let i = self.inner.lock().unwrap();

        match i.state {
            State::Rest => {
                self.emit(msg)?;
                self.emit("\r\n")?;
            }
            State::CleanedUp => {
                println!("{msg}");
            }
            State::Editing => {
                self.tx.send(Event::Log(msg.to_string()))?;
            }
        }

        Ok(())
    }

    fn emit(&self, msg: &str) -> Result<()> {
        let mut out = self.output.lock().unwrap();

        out.write_all(msg.as_bytes())?;
        out.flush()?;
        Ok(())
    }

    fn redraw_prompt(&self) -> Result<()> {
        let (prompt, buf) = {
            let i = self.inner.lock().unwrap();
            (i.prompt.to_string(), i.buffer.to_string())
        };
        self.emit("\r\x1b[0K")?;
        self.emit(&prompt)?;
        self.emit(&buf)?;
        Ok(())
    }

    fn log_while_editing(&self, msg: &str) -> Result<()> {
        /*
         * To print a log line while we are editing we need to move
         * the cursor back to the front of the line, clear
         * everything to our right, emit the line, then draw the
         * prompt and buffer contents right after.
         */
        self.emit("\r\x1b[0K")?;
        self.emit(msg)?;
        self.emit("\r\n")?;
        self.redraw_prompt()
    }

    pub fn line(&self) -> Result<Line> {
        let prompt = {
            let mut i = self.inner.lock().unwrap();
            match i.state {
                State::Rest => {
                    i.buffer.clear();
                    i.state = State::Editing;
                    i.prompt.to_string()
                }
                State::Editing => bail!("another thread is already editing"),
                State::CleanedUp => bail!("cleaned up already"),
            }
        };

        self.emit(&prompt)?;

        /*
         * Listen for input and process it.
         */
        let timeo = Duration::from_millis(500);
        loop {
            if self.sigterm.load(std::sync::atomic::Ordering::Relaxed) {
                /*
                 * Begin tearing down.
                 */
                self.cleanup();
                /*
                 * XXX
                 */
                return Ok(Line::End);
            }

            let b = match self.rx.lock().unwrap().recv_timeout(timeo) {
                Ok(Event::Input(b)) => b,
                Ok(Event::Interrupted) => continue,
                Ok(Event::Log(msg)) => {
                    self.log_while_editing(&msg)?;
                    continue;
                }
                Ok(Event::End) => {
                    /*
                     * XXX eof
                     */
                    self.cleanup();
                    return Ok(Line::End);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    /*
                     * XXX read thread problem?
                     */
                    self.cleanup();
                    return Ok(Line::End);
                }
            };

            if b.is_ascii_graphic() || b == b' ' {
                let mut i = self.inner.lock().unwrap();
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
                self.cleanup();
                return Ok(Line::End);
            } else if b == 0x04 {
                /*
                 * XXX ^D
                 */
                self.cleanup();
                return Ok(Line::End);
            } else if b == 0x0d {
                /*
                 * XXX CR
                 */
                let mut i = self.inner.lock().unwrap();
                i.state = State::Rest;
                let buf = i.buffer.clone();
                i.buffer.clear();
                self.emit("\r\n")?;
                return Ok(Line::Line(buf));
            } else if b == 0x7f {
                let mut i = self.inner.lock().unwrap();
                if !i.buffer.is_empty() {
                    let nl = i.buffer.len() - 1;
                    i.buffer.truncate(nl);
                    self.emit("\x08 \x08")?; /* XXX */
                }
            } else if b == 0x15 {
                /*
                 * XXX ^U
                 */
                self.inner.lock().unwrap().buffer.clear();
                self.redraw_prompt()?;
            } else {
                self.log_while_editing(&format!("unknown b: {b:?}"))?;

                /*
                 * Ring the bell!
                 */
                self.emit("\x07")?;
            }
        }
    }

    pub fn cleanup(&self) {
        {
            let mut i = self.inner.lock().unwrap();

            match i.state {
                State::CleanedUp => return,
                _ => i.state = State::CleanedUp,
            }
        }

        /*
         * Clean up the terminal and restore the original termios attributes:
         */
        let mut output = self.output.lock().unwrap();
        output.write_all(b"\r\n").ok();
        output.flush().ok();
        termios::tcsetattr(output.as_raw_fd(), TCSADRAIN, &self.orig_termios)
            .ok();
    }
}

impl Drop for Term {
    fn drop(&mut self) {
        self.cleanup();
    }
}
