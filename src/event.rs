use std::sync::Arc;
use colour::*;

pub mod colour {
	pub const BOLD: &str = "\x1b[1m";
	pub const ITALIC: &str = "\x1b[3m";
	pub const BRIGHT_BLACK: &str = "\x1b[90m";
	pub const RED: &str = "\x1b[31m";
	pub const RESET: &str = "\x1b[0m";
}

type Uname = Arc<str>;
type Msg   = Arc<str>;

#[derive(Clone, Debug)]
pub enum Event {
	Msg(Uname, Msg),
	Reply(Uname, Uname, Msg),

	Join(Uname),
	Leave(Uname),

	Terminate,
}

impl std::fmt::Display for Event {
	fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
		match self {
			Event::Msg(uname, msg) => write!(f, "{BOLD}{uname}{RESET}: {msg}"),
			Event::Join(uname)     => write!(f, "[{BOLD}{uname}{RESET} joined]"),
			Event::Leave(uname)    => write!(f, "[{BOLD}{uname}{RESET} left]"),
			Event::Reply(from, to, msg) => 
				write!(f, "{BOLD}{from}{RESET} {ITALIC}{BRIGHT_BLACK}to{RESET} {BOLD}{to}{RESET}: {msg}"),
			Event::Terminate => unreachable!(),
		}
	}
}
