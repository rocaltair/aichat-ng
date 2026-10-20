use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

pub type AbortSignal = Arc<AbortSignalInner>;

/// Process-wide record of a Ctrl+C.
///
/// SIGINT handling must not depend on which request-scoped future happens to be
/// polled: while a tool call runs — or a streaming reply is rendered without raw
/// mode — nothing awaits `tokio::signal::ctrl_c()`, so the terminal's default
/// disposition would kill the process with the whole conversation still in
/// memory. The handler installed by `install_ctrlc_handler` records the
/// interrupt here instead, and every `AbortSignal` observes it.
static INTERRUPTED: AtomicBool = AtomicBool::new(false);

pub fn interrupted() -> bool {
    INTERRUPTED.load(Ordering::SeqCst)
}

pub fn clear_interrupted() {
    INTERRUPTED.store(false, Ordering::SeqCst);
}

#[cfg(test)]
pub fn set_interrupted(value: bool) {
    INTERRUPTED.store(value, Ordering::SeqCst);
}

/// Install the process-wide Ctrl+C handler; call once from the CLI entry point.
/// Not used in serve mode, where SIGINT should stop the server.
pub fn install_ctrlc_handler() {
    tokio::spawn(async {
        loop {
            if tokio::signal::ctrl_c().await.is_err() {
                break;
            }
            INTERRUPTED.store(true, Ordering::SeqCst);
        }
    });
}

pub struct AbortSignalInner {
    ctrlc: AtomicBool,
    ctrld: AtomicBool,
}

pub fn create_abort_signal() -> AbortSignal {
    AbortSignalInner::new()
}

impl AbortSignalInner {
    pub fn new() -> AbortSignal {
        Arc::new(Self {
            ctrlc: AtomicBool::new(false),
            ctrld: AtomicBool::new(false),
        })
    }

    pub fn aborted(&self) -> bool {
        if self.aborted_ctrlc() {
            return true;
        }
        if self.aborted_ctrld() {
            return true;
        }
        false
    }

    pub fn aborted_ctrlc(&self) -> bool {
        self.ctrlc.load(Ordering::SeqCst) || interrupted()
    }

    pub fn aborted_ctrld(&self) -> bool {
        self.ctrld.load(Ordering::SeqCst)
    }

    pub fn reset(&self) {
        self.ctrlc.store(false, Ordering::SeqCst);
        self.ctrld.store(false, Ordering::SeqCst);
        clear_interrupted();
    }

    pub fn set_ctrlc(&self) {
        self.ctrlc.store(true, Ordering::SeqCst);
    }

    pub fn set_ctrld(&self) {
        self.ctrld.store(true, Ordering::SeqCst);
    }
}

pub async fn wait_abort_signal(abort_signal: &AbortSignal) {
    loop {
        if abort_signal.aborted() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

pub fn poll_abort_signal(abort_signal: &AbortSignal) -> Result<bool> {
    if crossterm::event::poll(Duration::from_millis(25))? {
        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Char('c') if key.modifiers == KeyModifiers::CONTROL => {
                    abort_signal.set_ctrlc();
                    return Ok(true);
                }
                KeyCode::Char('d') if key.modifiers == KeyModifiers::CONTROL => {
                    abort_signal.set_ctrld();
                    return Ok(true);
                }
                _ => {}
            }
        }
    }
    Ok(false)
}
