use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::terminal;
use itertools::{izip, Itertools};
use rand::prelude::*;
use std::io;
use std::iter;
use std::num::NonZeroUsize;
use structopt::StructOpt;
use tui::backend::CrosstermBackend;
use tui::layout::{Constraint, Direction, Layout, Rect};
use tui::style::{Color, Style};
use tui::widgets::{Paragraph, Text};
use tui::Frame;
use tui::Terminal;

static CIRCLE: &str = "●";
static DOT: &str = "∙";

static CODE_COLORS: &[Color] = &[
    Color::Blue,
    Color::Red,
    Color::Green,
    Color::Yellow,
    Color::Magenta,
    Color::White,
    Color::Cyan,
];
static BULL_COLOR: Color = Color::Red;
static COW_COLOR: Color = Color::White;

#[derive(Debug, StructOpt)]
#[structopt(
    name = env!("CARGO_PKG_NAME"),
    author = env!("CARGO_PKG_AUTHORS"),
    rename_all = "kebab-case",
    setting(clap::AppSettings::ColoredHelp),
    setting(clap::AppSettings::DeriveDisplayOrder),
    setting(clap::AppSettings::AllArgsOverrideSelf)
)]
struct Opt {
    /// Number of colors
    #[structopt(short, long, default_value = "6")]
    colors: NonZeroUsize,

    /// Maximum number of guesses
    #[structopt(short, long, default_value = "8")]
    guesses: NonZeroUsize,

    /// Number of holes per row
    #[structopt(short, long, default_value = "4")]
    holes: NonZeroUsize,

    /// Forbid colors to duplicate
    #[structopt(long)]
    no_duplicate: bool,
}

fn main() -> Result<()> {
    let opt = Opt::from_args();

    if opt.colors.get() > CODE_COLORS.len() {
        return Err(anyhow::anyhow!("--colors must be <= {}", CODE_COLORS.len()));
    }
    if opt.no_duplicate && opt.holes > opt.colors {
        return Err(anyhow::anyhow!(
            "--colors must be >= --holes when --no-duplicate"
        ));
    }

    Game::new(&opt).run()?;

    Ok(())
}

type Backend = CrosstermBackend<io::Stderr>;

#[derive(Debug, Clone, Default)]
struct Guess(Vec<usize>);

#[derive(Debug, Default, PartialEq)]
struct Hint {
    /// correct color, correct position
    bulls: usize,
    /// correct color, wrong position
    cows: usize,
}

#[derive(PartialEq)]
enum State {
    Playing,
    Won,
    Lost,
}

struct Game<'a> {
    opt: &'a Opt,
    solution: Guess,
    guesses: Vec<Guess>,
    hints: Vec<Hint>,
    current_guess: Guess,
}

impl<'a> Game<'a> {
    fn new(opt: &'a Opt) -> Game<'a> {
        let mut rng = rand::thread_rng();
        let solution = if opt.no_duplicate {
            // sample without replacement
            (0..opt.colors.get()).choose_multiple(&mut rng, opt.holes.get())
        } else {
            use rand::distributions::Uniform;

            // sample with replacement
            let dist = Uniform::new(0, opt.colors.get());
            rng.sample_iter(dist).take(opt.holes.get()).collect()
        };

        Self {
            opt,
            solution: Guess(solution),
            guesses: Vec::new(),
            hints: Vec::new(),
            current_guess: Guess(Vec::new()),
        }
    }

    fn run(&mut self) -> Result<()> {
        let (tx, rx) = crossbeam_channel::unbounded();
        std::thread::spawn(move || loop {
            if let Ok(event) = event::read() {
                let _ = tx.send(event);
            }
        });

        let mut terminal = setup_terminal()?;

        loop {
            terminal.draw(|mut f| {
                self.draw(&mut f);
            })?;

            if let Event::Key(key) = rx.recv()? {
                match (key.modifiers, key.code) {
                    (_, KeyCode::Esc)
                    | (KeyModifiers::CONTROL, KeyCode::Char('c'))
                    | (_, KeyCode::Char('q')) => break,
                    (_, KeyCode::Char(c)) if self.current_guess.0.len() < self.opt.holes.get() => {
                        if let Some(number) = parse_color_number(c) {
                            if number < self.opt.colors.get() {
                                self.current_guess.0.push(number);
                            }
                        }
                    }
                    (_, KeyCode::Backspace) | (KeyModifiers::CONTROL, KeyCode::Char('z')) => {
                        self.current_guess.0.pop();
                    }
                    (_, KeyCode::Enter) | (_, KeyCode::Char(' '))
                        if self.current_guess.0.len() == self.opt.holes.get() =>
                    {
                        let hint =
                            calc_hint(&self.current_guess, &self.solution, self.opt.colors.get());
                        self.guesses.push(std::mem::take(&mut self.current_guess));
                        self.hints.push(hint);

                        if self.status() != State::Playing {
                            terminal.draw(|mut f| {
                                self.draw(&mut f);
                            })?;
                            cleanup_terminal(&mut terminal)?;

                            return Ok(());
                        }
                    }
                    _ => (),
                }
            }
        }

        cleanup_terminal(&mut terminal)?;
        Ok(())
    }

    fn status(&self) -> State {
        if let Some(hint) = self.hints.last() {
            if hint.bulls == self.opt.holes.get() {
                return State::Won;
            }
        }

        if self.guesses.len() >= self.opt.guesses.get() {
            State::Lost
        } else {
            State::Playing
        }
    }

    fn draw(&self, f: &mut Frame<Backend>) {
        let board_height = self.opt.guesses.get()
            // solution row
            + 1
            // between board and message
            + 1;

        let chunks = Layout::default()
            .constraints([
                Constraint::Length(1),
                Constraint::Length(2),
                Constraint::Length(board_height as u16),
                Constraint::Length(1),
                Constraint::Min(1),
            ])
            .split(f.size());

        let text = vec![
            Text::styled(CIRCLE, Style::default().fg(BULL_COLOR)),
            Text::raw(" Correct color, correct position"),
        ];
        f.render_widget(Paragraph::new(text.iter()), chunks[0]);

        let text = vec![
            Text::styled(CIRCLE, Style::default().fg(COW_COLOR)),
            Text::raw(" Correct color, wrong position"),
        ];
        f.render_widget(Paragraph::new(text.iter()), chunks[1]);

        self.draw_board(f, chunks[2]);

        match self.status() {
            State::Playing => {
                let text = vec![if self.current_guess.0.len() < self.opt.holes.get() {
                    Text::raw("Press number keys to select colors")
                } else {
                    Text::raw("Press enter to make a guess")
                }];
                f.render_widget(Paragraph::new(text.iter()), chunks[3]);

                if !self.current_guess.0.is_empty() {
                    let text = vec![Text::raw("Press backspace to undo")];
                    f.render_widget(Paragraph::new(text.iter()), chunks[4]);
                }
            }
            State::Won => {
                let text = vec![Text::raw("You won!")];
                f.render_widget(Paragraph::new(text.iter()), chunks[3]);
            }
            State::Lost => {
                let text = vec![Text::raw("You lost")];
                f.render_widget(Paragraph::new(text.iter()), chunks[3]);
            }
        }
    }

    fn draw_board(&self, f: &mut Frame<Backend>, area: Rect) {
        let board_width = self.opt.holes.get() *
        (
            // codes
            2
            // keys
            + 1
        )
            // between codes and keys
            + 1
            // between keys and legend
            + 2;

        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(board_width as u16), Constraint::Min(1)])
            .split(area);

        let empty_guess = Default::default();
        let guesses = self
            .guesses
            .iter()
            .chain(iter::once(&self.current_guess))
            .chain(iter::repeat(&empty_guess))
            .take(self.opt.guesses.get());

        let empty_hint = Default::default();
        let hints = self
            .hints
            .iter()
            .chain(iter::repeat(&empty_hint))
            .take(self.opt.guesses.get());

        let constraints = vec![Constraint::Length(1); self.opt.guesses.get() + 1]; // +1 for solution
        let rows = Layout::default().constraints(constraints).split(chunks[0]);

        let solution_row = rows[0];
        let solution = if self.status() == State::Playing {
            &empty_guess
        } else {
            &self.solution
        };
        self.draw_row(f, &solution, None, solution_row);

        let rows = rows.iter().skip(1).rev();
        for (guess, hint, row) in izip!(guesses, hints, rows) {
            self.draw_row(f, guess, Some(hint), *row);
        }

        self.draw_legend(f, chunks[1]);
    }

    fn draw_row(&self, f: &mut Frame<Backend>, guess: &Guess, hint: Option<&Hint>, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(2 * self.opt.holes.get() as u16 + 1),
                Constraint::Min(1),
            ])
            .split(area);

        let text: Vec<_> = guess
            .0
            .iter()
            .map(|c| Text::styled(CIRCLE, Style::default().fg(CODE_COLORS[*c])))
            .chain(iter::repeat(Text::raw(DOT)))
            .take(self.opt.holes.get())
            .intersperse(Text::raw(" "))
            .collect();
        f.render_widget(Paragraph::new(text.iter()), chunks[0]);

        if let Some(hint) = hint {
            let bulls = iter::repeat(Text::styled(CIRCLE, Style::default().fg(BULL_COLOR)))
                .take(hint.bulls);
            let cows =
                iter::repeat(Text::styled(CIRCLE, Style::default().fg(COW_COLOR))).take(hint.cows);
            let dots = std::iter::repeat(Text::raw(DOT));

            let text: Vec<_> = bulls
                .chain(cows)
                .chain(dots)
                .take(self.opt.holes.get())
                .collect();
            f.render_widget(Paragraph::new(text.iter()), chunks[1]);
        }
    }

    fn draw_legend(&self, f: &mut Frame<Backend>, area: Rect) {
        let chunks = Layout::default()
            .constraints([Constraint::Length(1), Constraint::Min(1)])
            .split(area);

        let text: Vec<_> = (0..self.opt.colors.get())
            .map(|i| Text::raw((i + 1).to_string()))
            .intersperse(Text::raw(" "))
            .collect();
        f.render_widget(Paragraph::new(text.iter()), chunks[0]);

        let text: Vec<_> = CODE_COLORS
            .iter()
            .take(self.opt.colors.get())
            .map(|color| Text::styled(CIRCLE, Style::default().fg(*color)))
            .intersperse(Text::raw(" "))
            .collect();
        f.render_widget(Paragraph::new(text.iter()), chunks[1]);
    }
}

fn setup_terminal() -> Result<Terminal<Backend>> {
    terminal::enable_raw_mode()?;
    let backend = CrosstermBackend::new(io::stderr());
    let mut terminal = Terminal::new(backend)?;
    terminal.hide_cursor()?;
    terminal.clear()?;

    Ok(terminal)
}

fn cleanup_terminal(terminal: &mut Terminal<Backend>) -> Result<()> {
    terminal.show_cursor()?;
    terminal::disable_raw_mode()?;

    Ok(())
}

fn parse_color_number(c: char) -> Option<usize> {
    if let Some(digit) = c.to_digit(10) {
        if digit != 0 {
            return Some((digit - 1) as usize);
        }
    }
    None
}

fn calc_hint(guess: &Guess, solution: &Guess, num_colors: usize) -> Hint {
    let mut bulls = 0;
    let mut guess_counts = vec![0usize; num_colors];
    let mut solution_counts = vec![0usize; num_colors];
    for (guess, solution) in guess.0.iter().zip(solution.0.iter()) {
        if guess == solution {
            bulls += 1;
        } else {
            guess_counts[*guess] += 1;
            solution_counts[*solution] += 1;
        }
    }

    let cows = guess_counts
        .iter()
        .zip(solution_counts.iter())
        .fold(0, |sum, (a, b)| sum + a.min(b));

    Hint { bulls, cows }
}

#[cfg(test)]
#[macro_use]
extern crate quickcheck_macros;

#[cfg(test)]
mod tests {
    use super::*;
    use quickcheck::TestResult;

    #[allow(clippy::needless_range_loop)]
    fn gnome_mastermind_checkscores(guess: &Guess, solution: &Guess) -> Hint {
        let mut guess: Vec<_> = guess.0.iter().map(|x| *x as isize).collect();
        let solution: Vec<_> = solution.0.iter().map(|x| *x as isize).collect();
        let mut tmp = solution.clone();

        let mut bulls = 0;
        for i in 0..guess.len() {
            if guess[i] == solution[i] {
                bulls += 1;
                tmp[i] = -1;
                guess[i] = -2;
            }
        }

        let mut cows = 0;
        for i in 0..guess.len() {
            for j in 0..guess.len() {
                if guess[i] == tmp[j] {
                    cows += 1;
                    guess[i] = -2;
                    tmp[j] = -1;
                }
            }
        }

        Hint { bulls, cows }
    }

    #[quickcheck]
    fn hint(xs: Vec<(usize, usize)>) -> TestResult {
        if xs.is_empty() {
            return TestResult::discard();
        }

        let guess = Guess(xs.iter().map(|(a, _)| a).copied().collect());
        let solution = Guess(xs.iter().map(|(_, b)| b).copied().collect());
        let num_colors = guess.0.iter().chain(solution.0.iter()).max().unwrap() + 1;

        assert_eq!(
            calc_hint(&guess, &solution, num_colors),
            gnome_mastermind_checkscores(&guess, &solution)
        );

        TestResult::passed()
    }
}
