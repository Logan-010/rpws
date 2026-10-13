use crate::net::NetApi;
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use std::time::Duration;

fn app(term: &mut DefaultTerminal, _api: &NetApi) -> color_eyre::Result<()> {
    loop {
        term.draw(|f| f.render_widget("Hello — press q or Esc to quit", f.area()))?;
        if event::poll(Duration::from_millis(100))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
            && (matches!(key.code, KeyCode::Char('q') | KeyCode::Esc)
                || (key.code == KeyCode::Char('c')
                    && key.modifiers.contains(KeyModifiers::CONTROL)))
        {
            return Ok(());
        }
    }
}

pub fn run(api: NetApi) -> color_eyre::Result<()> {
    ratatui::run(|term| app(term, &api))
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore = "interactive terminal smoke test; press q to quit"]
    fn terminal_session() {
        super::run(crate::net::NetApi {}).unwrap();
    }
}
