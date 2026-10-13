use crate::net::NetApi;
use ratatui::DefaultTerminal;

fn app(term: &mut DefaultTerminal, api: &NetApi) -> color_eyre::Result<()> {
    term.draw(|f| f.render_widget("Hello", f.area()))?;

    Ok(())
}

pub fn run(api: NetApi) -> color_eyre::Result<()> {
    ratatui::run(|term| app(term, &api))
}
