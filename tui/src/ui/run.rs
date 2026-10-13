use crate::net::api::NetApi;
use ratatui::{
    DefaultTerminal,
    crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
};
use std::time::Duration;

fn app(term: &mut DefaultTerminal, _api: &NetApi) -> color_eyre::Result<()> {
    loop {
        term.draw(|f| {
            f.render_widget("Hello, world!", f.area());
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
                _ => {}
            }
        }
    }
}

pub fn run(api: NetApi) -> color_eyre::Result<()> {
    ratatui::run(|term| app(term, &api))
}
