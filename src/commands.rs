use std::sync::{Arc, RwLock};
use std::mem::{self, ManuallyDrop};
use std::path::Path;
use tokio::sync::MutexGuard;

use russh::server::Session;
use russh::{CryptoVec, ChannelId};

use crate::user::{User, UserConfig};
use crate::Event;
use crate::channel::{PermLevel, RestrictionKind};
use crate::channel::Channel;
use crate::event::colour::*;
use crate::SERVER;

pub enum CommandError {
	InvalidUtf8,
	InvalidArgs,
	InvalidPath,
	InvalidCommand,
	NotFound,
	AlreadyExists,
	Forbidden,
	Unimplemented,
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
			Self::AlreadyExists  => "EEXIST: Already exists",
			Self::Forbidden      => "EFRBD: Forbidden",
			Self::Unimplemented  => "EUNIMP: Not implemented",
		})
	}
}

impl crate::ChatClient {
	pub async fn command(
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
					all-users, ls                   - list all users in all channels\r\n\
					channel-perms, lsperm <name>    - list permissions for a channel\r\n\
					\r\n\
					passwd <pass>                   - change your password\r\n\
					\r\n\
					== Admin Commands ==\r\n\
					useradd <name>                  - create a new user\r\n\
					passwd-reset <name>             - reset a user's password\r\n";
				user.info(HELP).await;
			},
			["quit"] | ["q"] => {
				data!(b"\x1b[2K\r");
				Self::close(session, channel, user).await;
				return Ok(());
			},
			["clear"] => data!(b"\x1b[2J\x1b[H"),
			["reply", args @ ..] | ["r", args @ ..] => {
				let args = args.join(" "); 
				let (name, msg) = args.split_once(' ')
					.ok_or(CommandError::InvalidArgs)?;

				SERVER.read().users
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

				let name = Arc::from(*name);

				if SERVER.read().users.contains_key(&name) { Err(CommandError::AlreadyExists)?; }

				let pass = UserConfig::gen_pass();
				SERVER.write().users
					.insert(name, Arc::new(std::sync::Mutex::new(UserConfig::new(&pass[..]))));

				user.info(&pass[..]).await;
			},
			["passwd-reset", name] => {
				if user.config.lock().unwrap().get_global_perms() < PermLevel::MANAGE 
					{ Err(CommandError::Forbidden)?; }

				let pass = {
					let users = &mut SERVER.write().users;

					let Some(user) = users.get_mut(&Arc::from(*name)) 
						else { return Err(CommandError::NotFound); };

					let pass = UserConfig::gen_pass();
					user.lock().unwrap().hash = UserConfig::hash(&pass[..]);
					pass
				};

				user.info(&pass[..]).await;
			},
			["passwd", pass] => {
				user.config.lock().unwrap().hash = 
					crate::user::UserConfig::hash(pass.as_bytes());
			},
			["make-priv-channel", n @ ..] | ["mkchp", n @ ..] => {
				// TODO: create a priv channel
				todo!()
			},
			["mkch", path] => {
				let path = user.path.as_path().join(Path::new(path));

				let channel = path.parent()
					.and_then(|p| SERVER.read().channel_from_path(p))
					.ok_or(CommandError::InvalidPath)?;

				let name = path.file_name()
					.and_then(|n| n.to_str())
					.ok_or(CommandError::InvalidPath)?;

				let channels = &mut channel.write().unwrap().children;
				if channels.contains_key(name) { Err(CommandError::AlreadyExists)?; }

				let mut channel = Channel::new();

				channel.perms.push((RestrictionKind::All, PermLevel::READ|PermLevel::WRITE));
				channel.perms.push((RestrictionKind::User(user.name.clone()), PermLevel::READ|PermLevel::WRITE|PermLevel::MANAGE));

				channels.insert(Box::from(name), Arc::new(RwLock::new(channel)));
			},
			["remove-channel", path] | ["rmch", path] => {
				let path = user.path.as_path().join(Path::new(path));

				let channels = path.parent()
					.and_then(|p| SERVER.read().channel_from_path(p))
					.ok_or(CommandError::InvalidPath)?;

				let name = path.file_name()
					.and_then(|n| n.to_str())
					.ok_or(CommandError::InvalidPath)?;

				{
					let config = user.config.lock().unwrap();
					channels.read().unwrap().children.get(name)
						.ok_or(CommandError::NotFound)?
						.read().unwrap().perms.iter()
						.find(|(r, _)| match r {
							RestrictionKind::User(u) => *u == user.name,
							RestrictionKind::Role(r) => config.get_role(r).is_some(),
							RestrictionKind::All     => true })
						.map(|_| ())
						.or_else(|| (config.get_global_perms() > PermLevel::MANAGE).then_some(()))
						.ok_or(CommandError::Forbidden)?;
				}

				channels.write().unwrap()
					.children.remove(name).unwrap();
			},
			["channel", path] | ["ch", path] => {
				let path = user.path.as_path().join(Path::new(path));

				let channel = SERVER.read().channel_from_path(&path)
					.ok_or(CommandError::InvalidPath)?;

				user.path = path;

				mem::drop(mem::replace(&mut user.channel, Channel::subscribe(&channel)));
			},
			["pwch"] => {
				// SAFETY: info doesnt even get close to modyfying user path. 
				// SAFETY: &mut is needed so it can send data. 
				// SAFETY: we're just sidestepping the borrow checker to let it read the path directly
				let path = user.path.as_os_str().as_encoded_bytes() as *const _;
				user.info(unsafe { &*path }).await;
			},
			["lsch"] => {
				let thing = SERVER.read().channel_from_path(&user.path)
					.ok_or(CommandError::InvalidPath)?
					.read().unwrap().to_string();
				user.info(thing.as_bytes()).await;
			},
			["lsch", path] => {
				let path = user.path.as_path().join(Path::new(path));
				let thing = SERVER.read().channel_from_path(&path)
					.ok_or(CommandError::InvalidPath)?
					.read().unwrap().to_string();
				user.info(thing.as_bytes()).await;
			},
			["users"] | ["ls"] => {
				// TODO: list all users in current channel
				Err(CommandError::Unimplemented)?;
			},
			["all-users"] | ["lsa"] => {
				let userlist = SERVER.read().online_users
					.keys().fold(String::new(), |s, k| s + k + "\r\n");

				user.info(userlist.as_bytes()).await;
			},
			["user", name] => {
				// TODO: get user info
				Err(CommandError::Unimplemented)?;
			},
			["channel-perms", path] | ["lsperm", path] => {
				let path = user.path.as_path().join(Path::new(path));

				let channel = SERVER.read().channel_from_path(&path)
					.ok_or(CommandError::InvalidPath)?;

				let msg = format!("{:?}", channel.read().unwrap().perms);

				user.info(msg.as_bytes()).await;
			},
			_ => Err(CommandError::InvalidCommand)?,
		}

		data!(b"\x1b[2K\r");
		user.buf_clear();

		Ok(())
	}
}
