use std::time::Duration;
use std::sync::Arc;
use std::mem::{self, ManuallyDrop};
use tokio::sync::Mutex;

use russh::server::{Server as SshServer, Msg, Session, Handler, Auth};
use russh::{MethodSet, CryptoVec, ChannelId};
use russh_keys::key::KeyPair;
use colored::Colorize;

mod user;
mod channel;
mod event;
mod server;
mod config;
mod commands;

use user::{User, Connection, UserState};
use server::Server;
use event::Event;


pub const KEY_FILE: &str	= "key";
pub const STATE_FILE: &str  = "state.bin";

#[macro_export]
macro_rules! init {
	($dest:expr, $src:expr) => { mem::forget(mem::replace($dest, $src)) }
}


#[tokio::main]
async fn main() {
	let get_key_pair = || {
		use std::io::Read;

		let mut file = std::fs::File::open(KEY_FILE)
			.expect("err opening key file");

		let mut buf = [0; 64];
		file.read_exact(&mut buf)
			.expect("err reading key");

		KeyPair::Ed25519(
			ed25519_dalek::SigningKey::from_keypair_bytes(&buf)
				.expect("err parsing key"))
	};

	let config = russh::server::Config {
		inactivity_timeout:          Some(Duration::from_secs(3600)),
		auth_rejection_time:              Duration::from_secs(2),
		auth_rejection_time_initial: Some(Duration::from_secs(0)),
		keys:                        vec![get_key_pair()],
		methods:                     MethodSet::PASSWORD,
		..Default::default()
	};

	ChatClient::new()
		.run_on_address(Arc::new(config), "0.0.0.0:2222")
		.await.unwrap();
}

struct ChatClient {
	server: Arc<Server>,
	// ugly, but by far the best way to manually initialize the user
	user:   Arc<Mutex<ManuallyDrop<User>>>,
}

impl SshServer for ChatClient {
	type Handler = Self;
	#[allow(invalid_value)]
	fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self {
		Self {
			server: Arc::clone(&self.server),
			user:   Arc::new(Mutex::new(ManuallyDrop::new(unsafe { mem::MaybeUninit::zeroed().assume_init() }))),
		}
	}
}

impl Drop for ChatClient {
	fn drop(&mut self) {
		let user = &mut tokio::task::block_in_place(|| self.user.blocking_lock());

		// weak + strong ref take 2 words, meaning ptr is offset by 16 bytes
		if user.name.as_ref().as_ptr() as usize == mem::size_of::<usize>() * 2 { return; }

		self.server.online_users
			.lock().unwrap()
			.remove(&user.name);

		unsafe { ManuallyDrop::drop(user) }
	}
}

impl ChatClient {
	// SAFETY: zeroing an Arc is "UB", but we prevent it from dropping so its fine
	// SAFETY: fu Mai
	#[allow(invalid_value)] 
	fn new() -> Self {
		Self {
			server: Arc::new(
				std::fs::read(STATE_FILE)
					.inspect_err(|e| eprintln!("failed to open `{STATE_FILE}`: {e}"))
					.map(|s| bincode::deserialize(&s).unwrap_or_else(|e|
						panic!("failed to parse `{STATE_FILE}`: {e}")))
					.unwrap_or_default()),
			// user:   Arc::new(Mutex::new(ManuallyDrop::new(unsafe { mem::MaybeUninit::zeroed().assume_init() }))),
			// SAFETY: pretty sure the first instance is only there to init
			user: unsafe { mem::MaybeUninit::zeroed().assume_init() },
		}
	}
}

#[async_trait::async_trait]
impl Handler for ChatClient {
	type Error = russh::Error;

	async fn channel_open_session(
		&mut self,
		channel: russh::Channel<Msg>,
		session: &mut Session,
	) -> Result<bool, Self::Error> {
		let user = self.user.lock().await;

		// prob not gonna happen, but just in case
		if user.name.as_ref().as_ptr() as usize == mem::size_of::<usize>() * 2 { 
			return Err(russh::Error::NotAuthenticated); 
		}

		let conn = Connection::new(channel.id(), session.handle());
		let conf = user.config.clone();
		let name = user.name.clone();
		drop(user);

		// go online
		self.server.online_users
			.lock().unwrap()
			.insert(name.clone(), conf.clone());

		init!(
			&mut self.user,
			User::new(name.clone(), conf, self.server.clone(), conn)
		);

		let msg = CryptoVec::from_slice(b"Welcome! :help for commands, ctrl-c to exit.\r\n");
		session.handle().data(channel.id(), msg).await.unwrap();

		// can sometimes fail cause order of conn isnt guaranteed
		let _ = self.user.lock().await.channel
			.send(Event::Join(name)); 

		Ok(true)
	}

	async fn channel_close(
		&mut self, channel: ChannelId, session: &mut Session) 
	-> Result<(), Self::Error> {
		// FIXME: this gets called twice on quit for some reason
		let user = self.user.lock().await;
		user.channel.send(Event::Terminate).unwrap();
		user.channel.send(Event::Leave(user.name.clone())).unwrap();
		drop(user);

		session.data(channel, CryptoVec::from_slice(b"\r"));
		session.close(channel);
		Ok(())
	}

	async fn auth_password(&mut self, uname: &str, pass: &str) -> Result<Auth, Self::Error> {
		match tokio::task::block_in_place(|| self.server.validate_pass(uname, pass)) {
			Some(user) => {
				let mut usr = self.user.lock().await;
				init!(&mut usr.config, user.clone());
				init!(&mut usr.name,   Arc::from(uname));
				Ok(Auth::Accept)
			},
			_ => Ok(Auth::Reject {
				proceed_with_methods: Some(MethodSet::PASSWORD),
			}),
		}
	}

	async fn data(&mut self, channel: ChannelId, data: &[u8], session: &mut Session)
	-> Result<(), Self::Error> {
		macro_rules! data {
			($data:expr) => { session.data(channel, CryptoVec::from_slice($data)) }}

		let mut user = self.user.lock().await;

		match data {
			_ if matches!(user.state, UserState::Info(_)) => {
				let UserState::Info(data) =
				mem::replace(&mut user.state, UserState::Normal) 
				else { unreachable!(); };

				user.clear_info(&data).await;
			},

			[3] => { 
				drop(user);
				self.channel_close(channel, session).await.unwrap(); 
			},

			[13] => {
				if user.buffer.is_empty() { return Ok(()); }

				// FIXME: dont clone :p
				if let Some(buffer) = trim(&user.buffer.clone()).strip_prefix(b":") {
					if let Err(e) = self.command(channel, session, buffer, &mut user).await {
						// FIXME: even with this, channel_close is still called twice
						if e == "QUIT" { 
							drop(user);
							self.channel_close(channel, session).await.unwrap();
							return Ok(());
						}

						user.info(e.bold().red().to_string().as_bytes()).await;
						user.buf_clear();
					};
					return Ok(());
				}

				user.channel.send(Event::Msg(
					user.name.clone(),
					Arc::from(std::str::from_utf8(&user.buffer).unwrap()),
				)).unwrap();

				data!(b"\x1b[2K\r");

				user.buf_clear();
			},

			[127] => { // backsapce
				if user.cursor == 0 { return Ok(()); }

				let cursor = user.cursor;
				user.buffer.remove(cursor - 1);
				user.cursor -= 1;

				data!(b"\x1b[D\x1b[P");
			},

			[27, 91, 65] | // up arrow //TODO: replies
			[27, 91, 66]   // down arrow
			=> (),

			[27, 91, 67] => { // right arrow
				if user.cursor == user.buffer.len() { return Ok(()); }
				user.cursor += 1;
				data!(data);
			},

			[27, 91, 68] => { // left arrow
				if user.cursor == 0 { return Ok(()); }
				user.cursor -= 1;
				data!(data);
			},

			_ => {
				let cursor = user.cursor;
				user.buffer.splice(cursor..cursor, data.iter().cloned());
				user.cursor += data.len();

				data!(data);
			},
		} 
		Ok(())
	}
}

fn trim(data: &[u8]) -> &[u8] { unsafe { 
	data.get_unchecked(
		data.iter()
			.position(|&b| !(b as char).is_whitespace())
			.unwrap_or(data.len()) ..
		data.iter()
			.rposition(|&b| !(b as char).is_whitespace())
			.map_or(0, |i| i + 1))
}}
