use std::{
    collections::VecDeque,
    io::{self, Write},
    sync::{Arc, Mutex},
};
use tracing_subscriber::fmt::MakeWriter;

const MAX_LINES: usize = 1_000;
const MAX_EVENT_BYTES: usize = 16 * 1024;

#[derive(Default)]
struct State {
    ui_active: bool,
    lines: VecDeque<String>,
}

#[derive(Clone, Default)]
pub struct LogOutput(Arc<Mutex<State>>);

impl LogOutput {
    pub(super) fn capture(&self) -> CaptureGuard {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).ui_active = true;
        CaptureGuard(self.clone())
    }

    pub(super) fn lines(&self, offset: usize, count: usize) -> (usize, Vec<String>) {
        let state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let offset = offset.min(state.lines.len().saturating_sub(count));
        let mut lines: Vec<_> = state
            .lines
            .iter()
            .rev()
            .skip(offset)
            .take(count)
            .cloned()
            .collect();
        lines.reverse();
        (offset, lines)
    }
}

pub(super) struct CaptureGuard(LogOutput);

impl Drop for CaptureGuard {
    fn drop(&mut self) {
        self.0.0.lock().unwrap_or_else(|e| e.into_inner()).ui_active = false;
    }
}

pub struct LogWriter {
    output: LogOutput,
    bytes: Vec<u8>,
}

impl<'a> MakeWriter<'a> for LogOutput {
    type Writer = LogWriter;

    fn make_writer(&'a self) -> Self::Writer {
        LogWriter {
            output: self.clone(),
            bytes: Vec::new(),
        }
    }
}

impl Write for LogWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let remaining = MAX_EVENT_BYTES.saturating_sub(self.bytes.len());
        self.bytes
            .extend_from_slice(&bytes[..bytes.len().min(remaining)]);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for LogWriter {
    fn drop(&mut self) {
        let mut state = self.output.0.lock().unwrap_or_else(|e| e.into_inner());
        for line in String::from_utf8_lossy(&self.bytes).lines() {
            if state.lines.len() == MAX_LINES {
                state.lines.pop_front();
            }
            state
                .lines
                .push_back(line.chars().filter(|c| !c.is_control()).collect());
        }

        if !state.ui_active {
            let _ = io::stderr().lock().write_all(&self.bytes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_complete_events_and_restores_console_routing() {
        let output = LogOutput::default();
        let guard = output.capture();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .without_time()
            .with_writer(output.clone())
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!("first event");
            tracing::warn!("second event");
        });
        let (_, lines) = output.lines(0, 10);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("first event"));
        assert!(lines[1].contains("second event"));
        assert!(output.0.lock().unwrap().ui_active);
        drop(guard);
        assert!(!output.0.lock().unwrap().ui_active);
    }

    #[test]
    fn history_and_individual_events_are_bounded() {
        let output = LogOutput::default();
        let _guard = output.capture();
        for index in 0..MAX_LINES + 5 {
            writeln!(output.make_writer(), "line {index}").unwrap();
        }
        let (_, lines) = output.lines(0, MAX_LINES + 10);
        assert_eq!(lines.len(), MAX_LINES);
        assert_eq!(lines[0], "line 5");
        let (_, older) = output.lines(2, 2);
        assert_eq!(older, ["line 1001", "line 1002"]);
        let mut writer = output.make_writer();
        writer.write_all(&vec![b'x'; MAX_EVENT_BYTES * 2]).unwrap();
        assert_eq!(writer.bytes.len(), MAX_EVENT_BYTES);
    }
}
