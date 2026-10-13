use super::LogOutput;
use crate::net::NetApi;
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::{
    layout::{Constraint, Layout},
    text::Line,
    widgets::{Block, Paragraph},
};
use std::time::Duration;

fn app(term: &mut DefaultTerminal, _api: &NetApi, logs: &LogOutput) -> color_eyre::Result<()> {
    let mut offset: usize = 0;
    let mut page_size = 1;
    loop {
        term.draw(|f| {
            let [header, log_area] =
                Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(f.area());
            f.render_widget(
                "Hello — q/Esc: quit | PgUp/PgDn: logs | End: follow",
                header,
            );
            let block = Block::bordered().title("Logs");
            page_size = usize::from(block.inner(log_area).height).max(1);
            let (clamped_offset, lines) = logs.lines(offset, page_size);
            offset = clamped_offset;
            f.render_widget(
                Paragraph::new(lines.into_iter().map(Line::from).collect::<Vec<_>>()).block(block),
                log_area,
            );
        })?;
        if event::poll(Duration::from_millis(100))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Ok(());
                }
                KeyCode::PageUp => offset = offset.saturating_add(page_size),
                KeyCode::PageDown => offset = offset.saturating_sub(page_size),
                KeyCode::End => offset = 0,
                _ => {}
            }
        }
    }
}

pub fn run(api: NetApi, logs: LogOutput) -> color_eyre::Result<()> {
    let _capture = logs.capture();
    ratatui::run(|term| app(term, &api, &logs))
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore = "interactive terminal smoke test; press q to quit"]
    fn terminal_session() {
        let logs = super::LogOutput::default();
        tracing_subscriber::fmt()
            .with_ansi(false)
            .without_time()
            .with_writer(logs.clone())
            .try_init()
            .unwrap();
        let producer = std::thread::spawn(|| {
            for index in 0..5 {
                std::thread::sleep(std::time::Duration::from_millis(150));
                tracing::info!("live log {index}");
            }
        });
        super::run(crate::net::NetApi {}, logs).unwrap();
        producer.join().unwrap();
    }
}
