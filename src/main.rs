mod lsof;

use crate::lsof::Process;
use itertools::Itertools;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::symbols::border;
use ratatui::{DefaultTerminal, prelude::*};
use ratatui::widgets::{Block, HighlightSpacing, List, Row, Table, TableState};
use std::{env, io};
use std::process::Command;

fn main() -> io::Result<()> {
    let args = env::args().skip(1).join(" ");

    let mut app = App {
        processes: processes(),
        filter: args,
        ..App::default()
    };

    if app.filtered_list().count() == 1 {
        app.table.select(Some(0));
    }

    ratatui::run(|terminal| app.run(terminal))
}

#[derive(Debug, Default)]
enum State {
    #[default]
    ShowList,
    ShowHelp,
    EditFilter(String),
}

#[derive(Debug, Default)]
struct App {
    processes: Vec<Process>,
    exit: bool,
    table: TableState,
    filter: String,
    state: State,
}

impl App {
    /// runs the application's main loop until the user quits
    fn run(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        while !self.exit {
            terminal.draw(|frame| self.draw(frame))?;
            self.handle_events()?;
        }
        Ok(())
    }

    fn refresh_processes(&mut self) {
        self.processes = processes();
    }

    fn draw(&mut self, frame: &mut Frame) {
        frame.render_widget(self, frame.area());
    }

    fn handle_events(&mut self) -> io::Result<()> {
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
            State::ShowList => match key_event.code {
                KeyCode::Char('q') => self.exit(),
                KeyCode::Esc => self.handle_escape(),
                KeyCode::Up | KeyCode::Char('k') => self.table.select_previous(),
                KeyCode::Down | KeyCode::Char('j') => self.table.select_next(),
                KeyCode::Char('?') => self.state = State::ShowHelp,
                KeyCode::Char('/') => self.state = State::EditFilter(self.filter.clone()),
                KeyCode::Char('x') => self.kill_selected(),
                KeyCode::Char('r') => self.refresh_processes(),
                _ => {}
            },
            State::ShowHelp => match key_event.code {
                KeyCode::Char('q') => self.exit(),
                KeyCode::Esc | KeyCode::Char('?') => self.state = State::ShowList,
                _ => {}
            },
            State::EditFilter(filter) => match key_event.code {
                KeyCode::Enter => {
                    self.filter = filter.clone();
                    self.state = State::ShowList;
                }
                KeyCode::Esc => self.state = State::ShowList,
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
            State::ShowList | State::ShowHelp if !self.filter.is_empty() => {
                title.push(format!("/{}", self.filter).light_blue());
            }
            State::EditFilter(filter) => {
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
            let items = vec![p.pid.to_string(), p.command.to_string(), p.ports.join(",")];
            Row::new(items)
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
            .style(Style::new().white())
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
            State::ShowList | State::ShowHelp => &self.filter,
            State::EditFilter(f) => f,
        };

        self.processes
            .iter()
            .filter(|p| show_in_filter(p, filter))
    }
}

impl Widget for &mut App {
    fn render(self, area: Rect, buf: &mut Buffer) {
        match self.state {
            State::ShowList | State::EditFilter(_) => self.render_process_table(area, buf),
            State::ShowHelp => self.render_help(area, buf),
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
