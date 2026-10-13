use super::app::App;
use crate::net::api::NetApi;
use ratatui::{
    DefaultTerminal,
    crossterm::event::{self, Event, KeyEventKind},
};
use std::time::Duration;

fn app(term: &mut DefaultTerminal, api: NetApi) -> color_eyre::Result<()> {
    let mut app = App::new(api);
    app.refresh();
    loop {
        app.receive();
        term.draw(|frame| super::view::draw(frame, &mut app))?;
        if event::poll(Duration::from_millis(100))?
            && let Event::Key(key) = event::read()?
            && key.kind != KeyEventKind::Release
            && app.key(key)
        {
            return Ok(());
        }
    }
}

pub fn run(api: NetApi) -> color_eyre::Result<()> {
    ratatui::run(|term| app(term, api))
}
