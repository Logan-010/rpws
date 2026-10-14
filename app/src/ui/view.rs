use super::app::{App, CopyField, Form, Mode};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
};

const ACCENT: Color = Color::Cyan;

fn muted() -> Style {
    Style::default().fg(Color::DarkGray)
}
fn accent() -> Style {
    Style::default().fg(ACCENT)
}

pub(super) fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    if area.width < 36 || area.height < 12 {
        frame.render_widget(
            Paragraph::new("Enlarge the terminal to at least 36 × 12.\nCtrl+C to quit.")
                .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }
    let area = area.inner(ratatui::layout::Margin {
        horizontal: 2,
        vertical: 1,
    });
    let [header, body, status, help] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(2),
    ])
    .areas(area);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("rpws", accent().add_modifier(Modifier::BOLD)),
            Span::styled(format!("  /  {} passwords", app.entries.len()), muted()),
        ])),
        header,
    );

    if app.entries.is_empty() {
        let message = if app.busy() {
            "Loading your vault…"
        } else {
            "Your vault is empty.\nPress a to add your first password."
        };
        frame.render_widget(
            Paragraph::new(message)
                .style(muted())
                .block(Block::default().borders(Borders::TOP).border_style(muted())),
            body,
        );
    } else {
        let items: Vec<_> = app
            .entries
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                let mut lines = vec![Line::from(Span::styled(
                    clean(&entry.name),
                    Style::default().add_modifier(Modifier::BOLD),
                ))];
                if app.list.selected() == Some(index) && app.expanded {
                    lines.push(detail("user", &entry.payload.username));
                    lines.push(detail("url", &entry.payload.url));
                    lines.push(detail(
                        "password",
                        if app.revealed {
                            &entry.payload.password
                        } else {
                            "••••••••  (v to reveal)"
                        },
                    ));
                    if !entry.payload.extra.is_empty() {
                        lines.push(detail("notes", &entry.payload.extra));
                    }
                    lines.push(detail(
                        "updated",
                        &super::time::format_timestamp(entry.payload.timestamp),
                    ));
                    lines.push(Line::from(""));
                }
                ListItem::new(lines)
            })
            .collect();
        frame.render_stateful_widget(
            List::new(items)
                .block(Block::default().borders(Borders::TOP).border_style(muted()))
                .highlight_symbol("› ")
                .highlight_style(Style::default().fg(ACCENT)),
            body,
            &mut app.list,
        );
    }

    frame.render_widget(
        Paragraph::new(app.status.as_str()).style(if app.error {
            Style::default().fg(Color::Red)
        } else {
            muted()
        }),
        status,
    );
    let hints = match app.mode {
        Mode::Browse => {
            "↑/↓ select   enter expand   v reveal   c copy\na add   e edit   d delete   r refresh   q quit"
        }
        Mode::Form(_) => "tab / shift-tab field   ←/→ cursor\nctrl+s save   esc cancel",
        Mode::Delete(_) => "enter / y remove   esc / n cancel",
        Mode::Copy(_) => "↑/↓ select field   enter copy   esc cancel",
    };
    frame.render_widget(Paragraph::new(hints).style(muted()), help);
    let selected_name = app.selected().map(|entry| clean(&entry.name));
    let busy = app.busy();
    match &mut app.mode {
        Mode::Form(form) => draw_form(frame, area, form, busy),
        Mode::Delete(name) => {
            let popup = centered(area, 64, 8);
            frame.render_widget(Clear, popup);
            frame.render_widget(Paragraph::new(format!("Remove “{}” from the vault?\n\nDeletion syncs to peers; older copies will not restore it.\n\nEnter / y to remove · Esc to cancel", clean(name)))
                .wrap(Wrap { trim: false }).block(Block::bordered().title(" Remove password ").border_style(accent())), popup);
        }
        Mode::Copy(list) => {
            let popup = centered(area, 44, 11);
            frame.render_widget(Clear, popup);
            let block = Block::bordered()
                .title(" Copy to clipboard ")
                .border_style(accent());
            let inner = block.inner(popup);
            frame.render_widget(block, popup);
            let [name, fields, help] = Layout::vertical([
                Constraint::Length(1),
                Constraint::Min(6),
                Constraint::Length(1),
            ])
            .areas(inner);
            frame.render_widget(
                Paragraph::new(selected_name.unwrap_or_default()).style(muted()),
                name,
            );
            let items =
                CopyField::ALL.map(|field| ListItem::new(format!("Copy {}", field.label())));
            frame.render_stateful_widget(
                List::new(items)
                    .highlight_symbol("› ")
                    .highlight_style(accent()),
                fields,
                list,
            );
            frame.render_widget(
                Paragraph::new("Enter copy · Esc cancel").style(muted()),
                help,
            );
        }
        Mode::Browse => {}
    }
}

fn detail(label: &str, value: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("  {label:<9} "), muted()),
        Span::raw(if value.is_empty() {
            "—".into()
        } else {
            clean(value)
        }),
    ])
}

// Stored text must not introduce control characters or extra terminal rows.
fn clean(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}

fn draw_form(frame: &mut Frame, area: Rect, form: &Form, busy: bool) {
    let popup = centered(area, 72, 15);
    frame.render_widget(Clear, popup);
    let block = Block::bordered()
        .title(if form.editing {
            " Edit password "
        } else {
            " Add password "
        })
        .border_style(accent());
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    // Use one row per field on short terminals, two when space permits.
    let row_height = if inner.height >= 12 { 2 } else { 1 };
    let labels = ["Name", "URL", "Username", "Password", "Notes"];
    for (index, label) in labels.iter().enumerate() {
        let y = inner.y + index as u16 * row_height;
        if y >= inner.bottom() {
            break;
        }
        let focused = index == form.field;
        let field_area = Rect::new(inner.x, y, inner.width, 1);
        let label = format!(" {} {label:<9} ", if focused { "›" } else { " " });
        let prefix_width = 13u16;
        let width = inner.width.saturating_sub(prefix_width) as usize;
        let displayed: Vec<char> = form.fields[index]
            .chars()
            .map(|c| {
                if index == 3 {
                    '•'
                } else if c.is_control() {
                    ' '
                } else {
                    c
                }
            })
            .collect();
        let (start, offset) = if focused {
            field_window(&displayed, form.cursor, width)
        } else {
            (0, 0)
        };
        let value: String = displayed.iter().skip(start).collect();
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(label, if focused { accent() } else { muted() }),
                Span::raw(value),
            ])),
            field_area,
        );
        if focused && !busy && width > 0 {
            frame.set_cursor_position((
                inner.x + (prefix_width + offset as u16).min(inner.width.saturating_sub(1)),
                y,
            ));
        }
    }
    if inner.height > 5 * row_height {
        let text = form.error.as_deref().unwrap_or(if busy {
            "Saving…"
        } else if form.editing {
            "Name is fixed · Ctrl+S save · Esc cancel"
        } else {
            "Ctrl+S save · Esc cancel"
        });
        frame.render_widget(
            Paragraph::new(text).style(if form.error.is_some() {
                Style::default().fg(Color::Red)
            } else {
                muted()
            }),
            Rect::new(
                inner.x + 1,
                inner.bottom() - 1,
                inner.width.saturating_sub(2),
                1,
            ),
        );
    }
}

// Keep the insertion point visible, accounting for wide Unicode characters.
fn field_window(chars: &[char], cursor: usize, width: usize) -> (usize, usize) {
    let mut start = cursor;
    let mut offset = 0;
    while start > 0 {
        let char_width = Line::from(chars[start - 1].to_string()).width();
        if offset + char_width >= width {
            break;
        }
        start -= 1;
        offset += char_width;
    }
    (start, offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_scroll_keeps_wide_unicode_cursor_inside_input() {
        let chars: Vec<_> = "ab界🔑z".chars().collect();
        assert_eq!(field_window(&chars, 4, 5), (2, 4));
        assert_eq!(field_window(&chars, 5, 5), (3, 3));
        assert_eq!(field_window(&chars, 2, 10), (0, 2));
        assert_eq!(field_window(&chars, 5, 0), (5, 0));
    }
}
