use crate::net::api::NetApi;
use ratatui::{
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    widgets::ListState,
};
use shared::RecordPayload;
use std::{
    sync::{Arc, mpsc},
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::task::JoinHandle;

pub(super) struct Entry {
    pub name: String,
    pub payload: RecordPayload,
}

#[derive(Clone, Copy)]
pub(super) enum CopyField {
    Name,
    Url,
    Username,
    Password,
    Notes,
    Updated,
}

impl CopyField {
    pub const ALL: [Self; 6] = [
        Self::Name,
        Self::Url,
        Self::Username,
        Self::Password,
        Self::Notes,
        Self::Updated,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Url => "URL",
            Self::Username => "username",
            Self::Password => "password",
            Self::Notes => "notes",
            Self::Updated => "updated datetime",
        }
    }

    fn value(self, entry: &Entry) -> String {
        match self {
            Self::Name => entry.name.clone(),
            Self::Url => entry.payload.url.clone(),
            Self::Username => entry.payload.username.clone(),
            Self::Password => entry.payload.password.clone(),
            Self::Notes => entry.payload.extra.clone(),
            Self::Updated => super::time::format_timestamp(entry.payload.timestamp),
        }
    }
}

pub(super) struct Form {
    pub fields: [String; 5],
    pub field: usize,
    pub cursor: usize,
    pub editing: bool,
    pub error: Option<String>,
}

impl Form {
    fn new(entry: Option<&Entry>) -> Self {
        let fields = match entry {
            Some(entry) => [
                entry.name.clone(),
                entry.payload.url.clone(),
                entry.payload.username.clone(),
                entry.payload.password.clone(),
                entry.payload.extra.clone(),
            ],
            None => Default::default(),
        };
        let field = usize::from(entry.is_some());
        let cursor = fields[field].chars().count();
        Self {
            fields,
            field,
            cursor,
            editing: entry.is_some(),
            error: None,
        }
    }

    fn payload(&self) -> RecordPayload {
        RecordPayload {
            url: self.fields[1].clone(),
            username: self.fields[2].clone(),
            password: self.fields[3].clone(),
            extra: self.fields[4].clone(),
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        }
    }

    fn key(&mut self, key: KeyEvent) {
        let first = usize::from(self.editing);
        match key.code {
            KeyCode::Tab | KeyCode::Down | KeyCode::Enter => {
                self.field = if self.field == 4 {
                    first
                } else {
                    self.field + 1
                };
                self.cursor = self.fields[self.field].chars().count();
            }
            KeyCode::BackTab | KeyCode::Up => {
                self.field = if self.field == first {
                    4
                } else {
                    self.field - 1
                };
                self.cursor = self.fields[self.field].chars().count();
            }
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => {
                self.cursor = (self.cursor + 1).min(self.fields[self.field].chars().count())
            }
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.fields[self.field].chars().count(),
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                let offset = self.byte_offset();
                self.fields[self.field].insert(offset, c);
                self.cursor += 1;
            }
            KeyCode::Backspace if self.cursor > 0 => {
                self.cursor -= 1;
                let offset = self.byte_offset();
                self.fields[self.field].remove(offset);
            }
            KeyCode::Delete if self.cursor < self.fields[self.field].chars().count() => {
                let offset = self.byte_offset();
                self.fields[self.field].remove(offset);
            }
            _ => {}
        }
        self.error = None;
    }

    fn byte_offset(&self) -> usize {
        self.fields[self.field]
            .char_indices()
            .nth(self.cursor)
            .map_or(self.fields[self.field].len(), |(offset, _)| offset)
    }
}

pub(super) enum Mode {
    Browse,
    Form(Form),
    Delete(String),
    Copy(ListState),
}

enum Update {
    Loaded(Vec<Entry>),
    Saved(Entry),
    Deleted(String),
}

pub(super) struct App {
    api: Arc<NetApi>,
    pub entries: Vec<Entry>,
    pub list: ListState,
    pub mode: Mode,
    pub revealed: bool,
    pub expanded: bool,
    pub status: String,
    pub error: bool,
    pending: Option<(mpsc::Receiver<Result<Update, String>>, JoinHandle<()>)>,
    // Keep ownership alive: Linux clipboard contents are served by this handle.
    clipboard: Option<arboard::Clipboard>,
}

impl App {
    pub fn new(api: NetApi) -> Self {
        Self {
            api: Arc::new(api),
            entries: Vec::new(),
            list: ListState::default(),
            mode: Mode::Browse,
            revealed: false,
            expanded: true,
            status: String::new(),
            error: false,
            pending: None,
            clipboard: None,
        }
    }

    pub fn busy(&self) -> bool {
        self.pending.is_some()
    }

    pub fn selected(&self) -> Option<&Entry> {
        self.list
            .selected()
            .and_then(|index| self.entries.get(index))
    }

    fn start(
        &mut self,
        status: &str,
        future: impl Future<Output = color_eyre::Result<Update>> + Send + 'static,
    ) {
        let (sender, receiver) = mpsc::channel();
        let task = tokio::spawn(async move {
            let result = future.await.map_err(|error| error.to_string());
            let _ = sender.send(result);
        });
        self.pending = Some((receiver, task));
        self.status = status.into();
        self.error = false;
        self.revealed = false;
    }

    pub fn refresh(&mut self) {
        let api = self.api.clone();
        self.start("Loading passwords…", async move {
            Ok(Update::Loaded(
                api.list()
                    .await?
                    .into_iter()
                    .map(|(name, payload)| Entry { name, payload })
                    .collect(),
            ))
        });
    }

    pub fn receive(&mut self) {
        let Some((receiver, _)) = &self.pending else {
            return;
        };
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                Err("Network task stopped. Press r to retry.".into())
            }
        };
        self.pending = None;
        match result {
            Ok(update) => {
                let mut selected = self.selected().map(|entry| entry.name.clone());
                match update {
                    Update::Loaded(entries) => {
                        self.entries = entries;
                        self.status = "Passwords refreshed".into();
                    }
                    Update::Saved(entry) => {
                        selected = Some(entry.name.clone());
                        self.entries.retain(|old| old.name != entry.name);
                        self.entries.push(entry);
                        self.status = "Password saved".into();
                        self.mode = Mode::Browse;
                        self.expanded = true;
                    }
                    Update::Deleted(name) => {
                        self.entries.retain(|entry| entry.name != name);
                        self.status = "Password removed from local vault".into();
                        self.mode = Mode::Browse;
                    }
                }
                self.entries.sort_by(|a, b| a.name.cmp(&b.name));
                let index = selected
                    .and_then(|name| self.entries.iter().position(|entry| entry.name == name))
                    .unwrap_or(
                        self.list
                            .selected()
                            .unwrap_or(0)
                            .min(self.entries.len().saturating_sub(1)),
                    );
                self.list
                    .select((!self.entries.is_empty()).then_some(index));
            }
            Err(error) => {
                self.status = format!("Request failed: {error}");
                self.error = true;
            }
        }
    }

    // Returns true when the user asks to quit.
    pub fn key(&mut self, key: KeyEvent) -> bool {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return true;
        }
        if self.busy() {
            return matches!(self.mode, Mode::Browse)
                && matches!(key.code, KeyCode::Char('q') | KeyCode::Esc);
        }
        match &mut self.mode {
            Mode::Form(form) => {
                if key.code == KeyCode::Esc {
                    self.mode = Mode::Browse;
                } else if key.code == KeyCode::Char('s')
                    && key.modifiers.contains(KeyModifiers::CONTROL)
                {
                    let name = if form.editing {
                        form.fields[0].clone()
                    } else {
                        form.fields[0].trim().to_owned()
                    };
                    if name.is_empty() {
                        form.error = Some("Give this password a name.".into());
                    } else if !form.editing && self.entries.iter().any(|entry| entry.name == name) {
                        form.error =
                            Some("That name already exists. Edit its entry instead.".into());
                    } else {
                        let payload = form.payload();
                        let mut saved_payload = form.payload();
                        saved_payload.timestamp = payload.timestamp;
                        let saved = Entry {
                            name: name.clone(),
                            payload: saved_payload,
                        };
                        let api = self.api.clone();
                        self.start("Saving password…", async move {
                            api.put(name, payload).await?;
                            Ok(Update::Saved(saved))
                        });
                    }
                } else {
                    form.key(key);
                }
            }
            Mode::Delete(name) => match key.code {
                KeyCode::Char('y') | KeyCode::Enter => {
                    let name = name.clone();
                    let api = self.api.clone();
                    self.start("Removing password…", async move {
                        api.delete(&name).await?;
                        Ok(Update::Deleted(name))
                    });
                }
                KeyCode::Char('n') | KeyCode::Esc => self.mode = Mode::Browse,
                _ => {}
            },
            Mode::Copy(list) => match key.code {
                KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
                    list.select(Some(
                        (list.selected().unwrap_or(0) + 1) % CopyField::ALL.len(),
                    ));
                }
                KeyCode::Up | KeyCode::Char('k') | KeyCode::BackTab => {
                    list.select(Some(
                        (list.selected().unwrap_or(0) + CopyField::ALL.len() - 1)
                            % CopyField::ALL.len(),
                    ));
                }
                KeyCode::Enter => {
                    let field = CopyField::ALL[list.selected().unwrap_or(0)];
                    self.copy(field);
                }
                KeyCode::Char('q') | KeyCode::Esc => self.mode = Mode::Browse,
                _ => {}
            },
            Mode::Browse => match key.code {
                KeyCode::Char('q') | KeyCode::Esc => return true,
                KeyCode::Down | KeyCode::Char('j') => self.navigate(1),
                KeyCode::Up | KeyCode::Char('k') => self.navigate(-1),
                KeyCode::Home | KeyCode::Char('g') => self.select(0),
                KeyCode::End | KeyCode::Char('G') => {
                    self.select(self.entries.len().saturating_sub(1))
                }
                KeyCode::Enter | KeyCode::Char(' ') => {
                    self.expanded = !self.expanded;
                    self.revealed = false;
                }
                KeyCode::Char('v') => self.revealed = !self.revealed,
                KeyCode::Char('c') => {
                    if self.selected().is_some() {
                        self.revealed = false;
                        self.mode = Mode::Copy(ListState::default().with_selected(Some(0)));
                    }
                }
                KeyCode::Char('a') => {
                    self.revealed = false;
                    self.mode = Mode::Form(Form::new(None));
                }
                KeyCode::Char('e') => {
                    if let Some(entry) = self.selected() {
                        let form = Form::new(Some(entry));
                        self.revealed = false;
                        self.mode = Mode::Form(form);
                    }
                }
                KeyCode::Char('d') | KeyCode::Delete => {
                    if let Some(entry) = self.selected() {
                        let name = entry.name.clone();
                        self.revealed = false;
                        self.mode = Mode::Delete(name);
                    }
                }
                KeyCode::Char('r') => self.refresh(),
                _ => {}
            },
        }
        false
    }

    fn copy(&mut self, field: CopyField) {
        let Some(entry) = self.selected() else { return };
        let value = field.value(entry);
        let result = (|| {
            if self.clipboard.is_none() {
                self.clipboard = Some(arboard::Clipboard::new()?);
            }
            self.clipboard
                .as_mut()
                .expect("clipboard initialized")
                .set_text(value)
        })();
        match result {
            Ok(()) => {
                self.status = format!("Copied {} to clipboard", field.label());
                self.error = false;
                self.mode = Mode::Browse;
            }
            Err(error) => {
                self.status = format!("Could not copy {}: {error}", field.label());
                self.error = true;
            }
        }
    }

    fn navigate(&mut self, delta: isize) {
        let index = self
            .list
            .selected()
            .unwrap_or(0)
            .saturating_add_signed(delta)
            .min(self.entries.len().saturating_sub(1));
        self.select(index);
    }

    fn select(&mut self, index: usize) {
        self.list
            .select((!self.entries.is_empty()).then_some(index));
        self.revealed = false;
        self.expanded = true;
    }
}

impl Drop for App {
    fn drop(&mut self) {
        if let Some((_, task)) = self.pending.take() {
            task.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Router,
        body::Bytes,
        extract::State,
        http::StatusCode,
        routing::{delete, get, put},
    };
    use ratatui::{Terminal, backend::TestBackend};
    use std::{collections::HashMap, time::Duration};

    type Records = Arc<tokio::sync::Mutex<HashMap<String, RecordPayload>>>;

    async fn mock_list(State(records): State<Records>) -> Vec<u8> {
        postcard::to_stdvec(&*records.lock().await).unwrap()
    }

    async fn mock_put(State(records): State<Records>, body: Bytes) -> StatusCode {
        let (name, payload): (String, RecordPayload) = postcard::from_bytes(&body).unwrap();
        if payload.password == "reject" {
            return StatusCode::BAD_GATEWAY;
        }
        records.lock().await.insert(name, payload);
        StatusCode::NO_CONTENT
    }

    async fn mock_delete(State(records): State<Records>, body: Bytes) -> StatusCode {
        let name: String = postcard::from_bytes(&body).unwrap();
        records.lock().await.remove(&name);
        StatusCode::NO_CONTENT
    }

    fn press(app: &mut App, code: KeyCode) {
        app.key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    async fn settle(app: &mut App) {
        tokio::time::timeout(Duration::from_secs(3), async {
            while app.busy() {
                tokio::time::sleep(Duration::from_millis(5)).await;
                app.receive();
            }
        })
        .await
        .unwrap();
    }

    #[test]
    fn form_edits_unicode_without_corrupting_text_and_keeps_name_fixed() {
        let mut form = Form::new(None);
        for c in "aü🔑z".chars() {
            form.key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        form.key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        form.key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        form.key(KeyEvent::new(KeyCode::Char('é'), KeyModifiers::NONE));
        form.key(KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE));
        assert_eq!(form.fields[0], "aüé");
        let entry = Entry {
            name: "original".into(),
            payload: form.payload(),
        };
        let mut editing = Form::new(Some(&entry));
        editing.key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE));
        assert_eq!(editing.field, 4);
        editing.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(editing.field, 1);
        assert_eq!(editing.fields[0], "original");
    }

    #[test]
    fn copy_picker_navigates_cancels_and_preserves_raw_field_values() {
        let mut app = App::new(NetApi::new("http://127.0.0.1:1".into()));
        press(&mut app, KeyCode::Char('c'));
        assert!(matches!(app.mode, Mode::Browse));
        app.entries.push(Entry {
            name: "example".into(),
            payload: RecordPayload {
                url: "https://example.com".into(),
                username: "alice ü".into(),
                password: "secret-unique".into(),
                timestamp: 1,
                extra: "line one\nline two\t🔑".into(),
            },
        });
        app.list.select(Some(0));
        let entry = app.selected().unwrap();
        assert_eq!(CopyField::Name.value(entry), "example");
        assert_eq!(CopyField::Url.value(entry), "https://example.com");
        assert_eq!(CopyField::Username.value(entry), "alice ü");
        assert_eq!(CopyField::Password.value(entry), "secret-unique");
        assert_eq!(CopyField::Notes.value(entry), "line one\nline two\t🔑");
        assert_eq!(
            CopyField::Updated.value(entry),
            super::super::time::format_timestamp(1)
        );

        press(&mut app, KeyCode::Char('v'));
        press(&mut app, KeyCode::Char('c'));
        assert!(!app.revealed);
        press(&mut app, KeyCode::Up);
        assert!(matches!(&app.mode, Mode::Copy(list) if list.selected() == Some(5)));
        press(&mut app, KeyCode::Char('j'));
        assert!(matches!(&app.mode, Mode::Copy(list) if list.selected() == Some(0)));
        press(&mut app, KeyCode::Tab);
        assert!(matches!(&app.mode, Mode::Copy(list) if list.selected() == Some(1)));

        for (width, height) in [(80, 24), (36, 12)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| super::super::view::draw(frame, &mut app))
                .unwrap();
            let text: String = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            assert!(text.contains("Copy URL"));
            assert!(text.contains("Copy password"));
            assert!(!text.contains("secret-unique"));
        }
        press(&mut app, KeyCode::Esc);
        assert!(matches!(app.mode, Mode::Browse));
        assert_eq!(app.entries.len(), 1);
    }

    #[tokio::test]
    async fn ui_crud_uses_api_and_preserves_edits_on_failure() {
        let router = Router::new()
            .route("/list", get(mock_list))
            .route("/put", put(mock_put))
            .route("/delete", delete(mock_delete))
            .with_state(Records::default());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let api = NetApi::new(format!("http://{}", listener.local_addr().unwrap()));
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let mut app = App::new(api);
        app.refresh();
        settle(&mut app).await;
        assert!(app.entries.is_empty());
        press(&mut app, KeyCode::Char('a'));
        if let Mode::Form(form) = &mut app.mode {
            form.fields = [
                "example".into(),
                "https://example.com".into(),
                "alice".into(),
                "secret-unique".into(),
                "notes".into(),
            ];
        } else {
            panic!("missing add form");
        }
        app.key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
        settle(&mut app).await;
        assert!(!app.error);
        assert_eq!(app.selected().unwrap().payload.password, "secret-unique");

        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal
            .draw(|frame| super::super::view::draw(frame, &mut app))
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("alice"));
        assert!(text.contains(&super::super::time::format_timestamp(
            app.selected().unwrap().payload.timestamp
        )));
        assert!(!text.contains("secret-unique"));
        press(&mut app, KeyCode::Char('v'));
        terminal
            .draw(|frame| super::super::view::draw(frame, &mut app))
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("secret-unique"));

        press(&mut app, KeyCode::Char('e'));
        if let Mode::Form(form) = &mut app.mode {
            form.fields[3] = "reject".into();
        }
        app.key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
        settle(&mut app).await;
        assert!(app.error);
        assert_eq!(app.selected().unwrap().payload.password, "secret-unique");
        if let Mode::Form(form) = &mut app.mode {
            assert_eq!(form.fields[3], "reject");
            form.fields[3] = "updated".into();
        } else {
            panic!("failed save discarded the form");
        }
        app.key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
        settle(&mut app).await;
        app.refresh();
        settle(&mut app).await;
        assert_eq!(app.selected().unwrap().payload.password, "updated");

        press(&mut app, KeyCode::Char('d'));
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.entries.len(), 1);
        press(&mut app, KeyCode::Char('d'));
        press(&mut app, KeyCode::Enter);
        settle(&mut app).await;
        app.refresh();
        settle(&mut app).await;
        assert!(app.entries.is_empty());
        assert_eq!(app.list.selected(), None);
        server.abort();
    }
}
