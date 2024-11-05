use std::sync::Arc;

use colored::Colorize;

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
			Event::Msg(uname, msg) => write!(f, "{}: {msg}", uname.bold()),
			Event::Join(uname)	 => write!(f, "[{} joined]", uname.bold()),
			Event::Leave(uname)	=> write!(f, "[{} left]", uname.bold()),
			Event::Reply(from, to, msg) => 
				write!(f, "{} {} {}: {msg}", 
					from.bold(),
					"to".italic().bright_black(),
					to.bold()),
			Event::Terminate	   => unreachable!(),
		}
	}
}
