use std::sync::Arc;
use std::mem::{self, ManuallyDrop};
use tokio::sync::MutexGuard;

use russh::server::Session;
use russh::{CryptoVec, ChannelId};

use colored::Colorize;

use crate::user::User;
use crate::server::Server;
use crate::Event;
use crate::channel::{PermLevel, RestrictionKind};
use crate::channel::Channel;
use crate::event::colour::*;

pub enum CommandError {
	InvalidUtf8,
	InvalidArgs,
	InvalidPath,
	InvalidCommand,
	NotFound,
	AlreadyExists,
	Forbidden,
	Unimplemented,
	Other(String),
}

use std::fmt;
impl fmt::Display for CommandError {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "{RED}{BOLD}{}{RESET}", match self {
			Self::InvalidUtf8    => "EUTF: Invalid utf8",
			Self::InvalidArgs    => "EBADA: Invalid arguments",
			Self::InvalidPath    => "EIPATH: Invalid Path",
			Self::InvalidCommand => "EINVAL: Invalid command",
			Self::NotFound       => "ENFOUND: Not found",
			Self::AlreadyExists  => "EEXIST: User already exists",
			Self::Forbidden      => "EFRBD: Forbidden",
			Self::Unimplemented  => "EUNIMP: Not implemented",
			Self::Other(e)       => e,
		})
	}
}

impl crate::ChatClient {
	pub async fn command(&self, 
		channel: ChannelId, 
		session: &mut Session,
		data: &[u8],
		user: &mut MutexGuard<'_, ManuallyDrop<User>>)
	-> Result<(), CommandError> {
		macro_rules! data {
			($data:expr) => { session.data(channel, CryptoVec::from_slice($data)) }}

		let cmd = std::str::from_utf8(data)
			.map_err(|_| CommandError::InvalidUtf8)?
			.split(' ').collect::<Vec<_>>();

		let channel_from_path = |path: &[&str], offset| match path.first() {
			Some(&"") => Server::channel_from_path(&path[1..(path.len() as isize + offset) as usize], self.server.channels.clone()),
			Some(_)   => Server::channel_from_path(&path[ ..(path.len() as isize + offset) as usize], user.channel.children.clone()),
			None      => Err(None),
		};

		match cmd.as_slice() {
			["help"] | ["h"] => {
				const HELP: &[u8] = b"\
					== Commands ==\r\n\
					help, h         - show this message\r\n\
					clear           - clear the terminal\r\n\
					quit, q         - close the connection\r\n\
					reply, r <name> - reply a message from <name>\r\n\
					\r\n\
					make-channel, mkch <name>       - create a new public channel\r\n\
					make-priv-channel, mkchp <name> - create a new private channel\r\n\
					remove-channel, rmch <name>     - remove a channel\r\n\
					channel, ch <name>              - move to a channel\r\n\
					channels, lsc                   - list all channels\r\n\
					users, lsu                      - list all users in the current channel\r\n\
					all-users, lsU                  - list all users in all channels\r\n";
				user.info(HELP).await;
			},
			["quit"] | ["q"] => {
				data!(b"\x1b[2K\r");
				self.close(session, channel, user).await;
				return Ok(());
			},
			["clear"] => data!(b"\x1b[2J\x1b[H"),
			["reply", args @ ..] | ["r", args @ ..] => {
				let args = args.join(" "); 
				let (name, msg) = args.split_once(' ')
					.ok_or(CommandError::InvalidArgs)?;

				self.server.users.lock().unwrap()
					.contains_key(name).then_some(())
					.ok_or(CommandError::NotFound)?;

				user.channel.send(
					Event::Reply(user.name.clone(), Arc::from(name), Arc::from(msg)))
					.unwrap();
			},
			["useradd", name] => {
				if user.config.lock().unwrap().get_global_perms() < PermLevel::MANAGE { 
					Err(CommandError::Forbidden)?;
				}

				let pass = { // ugly but rust cant comprehend that drop() unlocks a mutex
					let name = Arc::from(*name);
					let mut users = self.server.users.lock().unwrap();

					if users.contains_key(&name) { Err(CommandError::AlreadyExists)?; }

					let (conf, pass) = crate::user::UserConfig::new();
					users.insert(name, Arc::new(std::sync::Mutex::new(conf)));
					pass
				};

				user.info(&pass[..]).await;
			},
			["passwd", pass] => {
				user.config.lock().unwrap().hash = 
					crate::user::UserConfig::hash(pass.as_bytes());
			},
			["make-priv-channel", n @ ..] | ["mkchp", n @ ..] => {
				// create a global channel
				todo!()
			},
			["make-channel", path] | ["mkch", path] => {
				let path = path.split('/').collect::<Vec<_>>();
				let Err(Some(channels)) = channel_from_path(&path, -1) 
					else { return Err(CommandError::InvalidPath); }; // grrr >:( 
				let name = path.last().unwrap();

				let mut channels = channels.write().unwrap();

				if channels.iter().any(|c| &*c.name == *name) {
					Err(CommandError::AlreadyExists)?;
				}

				let channel = crate::channel::Channel::new(Arc::from(*name));
				{
					let mut channel = channel.perms.write().unwrap();
					channel.push((RestrictionKind::All, PermLevel::READ|PermLevel::WRITE));
					channel.push((RestrictionKind::User(user.name.clone()), PermLevel::READ|PermLevel::WRITE|PermLevel::MANAGE));
				}

				channels.push(channel);
			},
			["remove-channel", path] | ["rmch", path] => {
				let path = path.split('/').collect::<Vec<_>>();

				let (channels, index) = channel_from_path(&path, 0)
					.map_err(|_| CommandError::InvalidPath)?;

				{
					let config = user.config.lock().unwrap();
					channels.read().unwrap()[index].perms.read().unwrap().iter()
						.find(|(r, p)| match r {
							RestrictionKind::User(u) => p.contains(PermLevel::MANAGE) && *u == user.name,
							RestrictionKind::Role(r) => config.get_role(r).filter(|p| p.contains(PermLevel::MANAGE)).is_some(),
							RestrictionKind::All     => p.contains(PermLevel::MANAGE) })
						.map(|_| ())
						.or_else(|| (config.get_global_perms() > PermLevel::MANAGE).then_some(()))
						.ok_or(CommandError::Forbidden)?;
				}

				let mut channels = channels.write().unwrap();
				channels.remove(index);
			},
			["channel", path] | ["ch", path] => {
				let path = path.split('/').collect::<Vec<_>>();
				let (channels, index) = channel_from_path(&path, 0)
					.map_err(|_| CommandError::InvalidPath)?;

				mem::drop(mem::replace(&mut user.channel, channels.read().unwrap()[index].clone().subscribe()));
			},
			["channels"] | ["ls"] => {
				use std::io::Write;
				let tree = self.server.channels
					.read().unwrap()
					.iter().try_fold(Vec::new(), |mut acc, c| {
						write!(acc, "{c}\r")?; Ok(acc) })
					.map_err(|e: std::io::Error| CommandError::Other(e.to_string()))?;

				user.info(&tree).await;
			},
			["users"] | ["lsu"] => {
				// list all users in current channel
				Err(CommandError::Unimplemented)?;
			},
			["all-users"] | ["lsU"] => {
				let userlist = self.server.online_users.lock().unwrap()
					.keys().fold(String::new(), |s, k| s + k + "\r\n");

				user.info(userlist.as_bytes()).await;
			},
			["channel-perms", path] | ["lsp", path] => {
				let path = path.split('/').collect::<Vec<_>>();
				let (channels, index) = channel_from_path(&path, 0)
					.map_err(|_| CommandError::InvalidPath)?;

				let msg = {
					let channel = &channels.read().unwrap()[index];
					format!("{:?}", channel.perms.read().unwrap())
				};

				user.info(msg.as_bytes()).await;
			},
			_ => Err(CommandError::InvalidCommand)?,
		}

		data!(b"\x1b[2K\r");
		user.buf_clear();

		Ok(())
	}
}
