mod lsof;

use crate::lsof::Process;
use itertools::Itertools;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::symbols::border;
use ratatui::widgets::{Block, HighlightSpacing, List, Row, Table, TableState};
use ratatui::{DefaultTerminal, prelude::*};
use std::process::Command;
use std::sync::mpsc::{Receiver, sync_channel};
use std::time::Duration;
use std::{env, io, thread};

const UPDATE_INTERVAL: Duration = Duration::from_millis(500);

fn main() -> io::Result<()> {
    let args = env::args().skip(1).join(" ");

    let receiver = spawn_process_updater();

    let mut app = App {
        filter: args,
        receiver,
        processes: processes(),
        exit: false,
        table: TableState::default(),
        state: AppState::default(),
    };

    ratatui::run(|terminal| app.run(terminal))
}

/// Spawn a thread for updating the list of processes.
/// Returns a receiver for receiving the updates.
fn spawn_process_updater() -> Receiver<Vec<Process>> {
    let (sender, receiver) = sync_channel(0);

    thread::spawn(move || {
        loop {
            let procs = processes();
            if sender.send(procs).is_err() {
                break;
            }
        }
    });

    receiver
}

#[derive(Debug, Default)]
enum AppState {
    #[default]
    ShowList,
    ShowHelp,
    EditFilter(String),
}

#[derive(Debug)]
struct App {
    /// The complete list of processes.
    /// Prefer to use filtered_list for UI purposes.
    processes: Vec<Process>,
    exit: bool,
    table: TableState,
    filter: String,
    state: AppState,
    receiver: Receiver<Vec<Process>>,
}

impl App {
    /// runs the application's main loop until the user quits
    fn run(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        while !self.exit {
            self.refresh_processes();
            terminal.draw(|frame| self.draw(frame))?;
            self.handle_events()?;
        }
        Ok(())
    }

    fn refresh_processes(&mut self) {
        // To keep a stable selection, we will remember the PID of the selected process
        // before updating and restore it after.
        let selected_pid = self
            .table
            .selected()
            .and_then(|i| self.filtered_list().nth(i))
            .map(|p| p.pid);

        // We expect a value to be in the channel, no waiting.
        if let Ok(procs) = self.receiver.recv_timeout(Duration::ZERO) {
            self.processes = procs;
        }

        if let Some(selected_pid) = selected_pid {
            let i = self.filtered_list().position(|p| p.pid == selected_pid);
            self.table.select(i);
        }
    }

    fn draw(&mut self, frame: &mut Frame) {
        frame.render_widget(self, frame.area());
    }

    fn handle_events(&mut self) -> io::Result<()> {
        let event_available = event::poll(UPDATE_INTERVAL)?;
        if !event_available {
            return Ok(());
        }

        match event::read()? {
            Event::Key(key_event) if key_event.kind == KeyEventKind::Press => {
                self.handle_key_event(key_event)
            }
            _ => {}
        };
        Ok(())
    }

    fn handle_key_event(&mut self, key_event: KeyEvent) {
        match &mut self.state {
            AppState::ShowList => match key_event.code {
                KeyCode::Char('q') => self.exit(),
                KeyCode::Esc => self.handle_escape(),
                KeyCode::Up | KeyCode::Char('k') => self.table.select_previous(),
                KeyCode::Down | KeyCode::Char('j') => self.table.select_next(),
                KeyCode::Char('?') => self.state = AppState::ShowHelp,
                KeyCode::Char('/') => self.state = AppState::EditFilter(self.filter.clone()),
                KeyCode::Char('x') => self.kill_selected(),
                _ => {}
            },
            AppState::ShowHelp => match key_event.code {
                KeyCode::Char('q') => self.exit(),
                KeyCode::Esc | KeyCode::Char('?') => self.state = AppState::ShowList,
                _ => {}
            },
            AppState::EditFilter(filter) => match key_event.code {
                KeyCode::Enter => {
                    self.filter = filter.clone();
                    self.state = AppState::ShowList;
                }
                KeyCode::Esc => self.state = AppState::ShowList,
                KeyCode::Backspace => {
                    filter.pop();
                }
                key => edit_filter_text(filter, key),
            },
        }
    }

    fn exit(&mut self) {
        self.exit = true;
    }

    fn render_process_table(&mut self, area: Rect, buf: &mut Buffer) {
        let mut title = vec![" Processes ".bold()];

        match &self.state {
            AppState::ShowList | AppState::ShowHelp if !self.filter.is_empty() => {
                title.push(format!("/{}", self.filter).light_blue());
            }
            AppState::EditFilter(filter) => {
                title.push(format!("/{filter}").black().on_light_blue());
            }
            _ => (),
        }

        let title = Line::from(title);
        let block = Block::new()
            .title(title.centered())
            .title_bottom(Line::from("<q> or <esc> to quit. <x> to kill. <?> for help.").centered())
            .style(Style::new().white());

        let rows = self.filtered_list().map(|p| {
            Row::new(vec![
                format!("{:>5}", p.pid),
                p.command.to_string(),
                p.ports.join(","),
            ])
        });

        let header = Row::new(vec!["PID", "Command", "Ports"]).style(Style::new().bold());

        let columns = [
            Constraint::Length(8),
            Constraint::Fill(1),
            Constraint::Fill(1),
        ];

        let table = Table::new(rows, columns)
            .block(block)
            .header(header)
            .highlight_symbol(">")
            .highlight_spacing(HighlightSpacing::Always)
            .row_highlight_style(Style::new().light_red().bold());

        StatefulWidget::render(table, area, buf, &mut self.table);
    }

    fn render_help(&self, area: Rect, buf: &mut Buffer) {
        let title = Line::from(" Help ".bold());
        let block = Block::bordered()
            .title(title.centered())
            .border_set(border::ROUNDED);

        let items = [
            "<q> Quit",
            "<esc> Clear filter or quit",
            "<k> or <↑> Select previous",
            "<j> or <↓> Select next",
            "<x> Kill selected",
            "<r> Refresh list",
            "</> Filter",
        ];

        let list = List::new(items).block(block);
        Widget::render(list, area, buf);
    }

    fn kill_selected(&mut self) {
        let Some(selected) = self.table.selected() else {
            return;
        };

        let Some(selected) = self.filtered_list().nth(selected) else {
            return;
        };

        kill(selected.pid);
        self.refresh_processes();
    }

    fn handle_escape(&mut self) {
        if self.filter.is_empty() {
            self.exit();
        } else {
            self.filter.clear();
        }
    }

    fn filtered_list(&self) -> impl Iterator<Item = &Process> {
        let filter = match &self.state {
            AppState::ShowList | AppState::ShowHelp => &self.filter,
            AppState::EditFilter(f) => f,
        };

        self.processes.iter().filter(|p| show_in_filter(p, filter))
    }
}

impl Widget for &mut App {
    fn render(self, area: Rect, buf: &mut Buffer) {
        match self.state {
            AppState::ShowList | AppState::EditFilter(_) => self.render_process_table(area, buf),
            AppState::ShowHelp => self.render_help(area, buf),
        }
    }
}

fn show_in_filter(p: &Process, filter: &str) -> bool {
    p.command.contains(filter)
        || p.ports.iter().any(|port| port.contains(filter))
        || p.pid.to_string().contains(filter)
}

fn edit_filter_text(filter: &mut String, key: KeyCode) {
    let Some(c) = key.as_char() else {
        return;
    };

    filter.push(c);
}

fn kill(pid: usize) {
    Command::new("kill").arg(pid.to_string()).output().unwrap();
}

fn processes() -> Vec<Process> {
    lsof::lsof()
        .into_iter()
        .filter(|p| !p.ports.is_empty())
        .collect()
}
