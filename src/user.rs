use std::sync::{Arc, Weak, Mutex};
use std::mem::ManuallyDrop;
use std::path::{PathBuf, Path};
use tokio::task::{self, JoinHandle};
use tokio::sync::Mutex as AsyncMutex;
use tokio::sync::broadcast::error::TryRecvError;

use serde::Deserialize;
use russh::CryptoVec;

use crate::server::Server;
use crate::channel::{SubscribedChannel, PermLevel};
use crate::event::Event;


pub struct User {
   pub name:    Arc<str>,
   pub buffer:  Vec<u8>,
   pub cursor:  usize,
   pub state:   UserState,

   pub config:  UserConfLock,
   conn:        Connection,

   handle:      JoinHandle<()>,

   pub channel: SubscribedChannel,
	pub path:    PathBuf,
}

// condvar to save config changes
pub type UserConfLock = Arc<Mutex<UserConfig>>;

#[derive(Deserialize, serde::Serialize)]
pub struct UserConfig {
   #[serde(deserialize_with = "UserConfig::deserialize_hash")]
   pub hash:  u64,
   pub roles: Vec<(Box<str>, PermLevel)>,
}

impl UserConfig {
   pub fn new(pass: &[u8]) -> Self {
      Self { 
         hash: UserConfig::hash(pass),
         roles: Vec::new(),
      }
   }

	pub fn gen_pass() -> [u8; 8] {
      use rand::Rng;
      let mut pass = [0; 8];
      rand::thread_rng()
         .sample_iter(&rand::distributions::Alphanumeric)
         .zip(pass.iter_mut())
         .for_each(|(c, b)| *b = c);
		pass
	}

   fn deserialize_hash<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
      use serde::de::Error;
      let s = String::deserialize(d)?;
      u64::from_str_radix(&s, 16).map_err(D::Error::custom)
   }

   pub fn hash(pass: &[u8]) -> u64 {
      use std::hash::{Hash, Hasher};
      let mut hasher = std::collections::hash_map::DefaultHasher::new();
      pass.hash(&mut hasher);
      hasher.finish()
   }

   pub fn get_role(&self, name: &str) -> Option<PermLevel> {
      self.roles.iter().find_map(|(n, p)| (&**n == name).then_some(*p))
   }

   pub fn get_global_perms(&self) -> PermLevel {
      self.roles.iter().fold(PermLevel::NONE, |acc, (_, p)| acc | *p)
   }
}

pub enum UserState {
   Info(Box<[u8]>),
   Normal,
}

impl User {
   pub fn new(name: Arc<str>, config: UserConfLock, server: Arc<Server>, conn: Connection) -> Arc<AsyncMutex<ManuallyDrop<Self>>> {
		Arc::new_cyclic(|user|
			AsyncMutex::new(ManuallyDrop::new(Self { 
				name, config, conn, 
				path: PathBuf::from("/"), // TODO: save user's current channel
				handle: task::spawn(Self::event_loop(user.clone())),
				channel: crate::channel::Channel::subscribe(
					&server.channel_from_path(&Path::new("/"))
						.expect("default channel does not exist")),
				buffer: Vec::with_capacity(256),
				cursor: 0,
				state: UserState::Normal,
			})))
   }

   async fn event_loop(user: Weak<AsyncMutex<ManuallyDrop<Self>>>) {
		let notify = match user.upgrade() {
			Some(user) => user.lock().await.channel.notify.clone(),
			None => panic!("User dropped on init!"),
		};

      loop {
			let mut user = match user.upgrade() {
				Some(user) => user.lock_owned().await,
				None => {
					eprintln!("User dropped!");
					break;
				},
			};

         let event = match user.channel.rx.try_recv() {
            Err(TryRecvError::Empty) => {
					std::mem::drop(user);
					notify.notified().await;
					continue;
				},
				// TODO we could just drop the Arc (and as a matter of fact we do)
				// but this is here so we're certain the user burns through all of the event queue
				// before quit. Might not be necessary given that you dont really care to get all the
				// events if you're quitting anyway.
            Ok(Event::Terminate) => break, 
            Ok(event) => event,
            Err(TryRecvError::Closed) => unreachable!(),
            Err(TryRecvError::Lagged(num)) => {
               #[cfg(debug_assertions)]
               eprintln!("Event lagged");

               user.conn.data(CryptoVec::from(format!("ECHL: Channel Lost Events: {num}\r\n"))).await;
               continue;
            },
         };

         match user.state {
            UserState::Normal if !user.buffer.is_empty() => {
               user.conn.data(CryptoVec::from_slice(b"\x1b[2K\r")).await;
               user.conn.data(CryptoVec::from(format!("{event}\r\n"))).await;
               user.conn.data(CryptoVec::from_slice(&user.buffer)).await;
            },
            UserState::Normal => {
               user.conn.data(CryptoVec::from(format!("{event}\r\n"))).await;
				},
            UserState::Info(ref data) => {
               user.clear_info(data).await;
               user.conn.data(CryptoVec::from(format!("{event}\r\n"))).await;
               user.conn.data(CryptoVec::from_slice(data)).await;
               user.conn.data(CryptoVec::from_slice(&user.buffer)).await;
            },
         }
      }
   }

   pub async fn clear_info(&self, data: &[u8]) {
      match data.iter().filter(|&&b| b == b'\n').count() {
         0 => self.conn.data(CryptoVec::from_slice(b"\x1b[2K\r")).await.unwrap(),
         // for some reason sending the same bytes 10x in a row caused a deadlock
         c => self.conn.data((0..c).fold(
            CryptoVec::with_capacity(8*c),
            |mut v, _| { v.extend(b"\x1b[1F\x1b[2K"); v }
         )).await.unwrap(),
      }
   }

   pub async fn info(&mut self, data: &[u8]) {
      self.state = UserState::Info(Box::from(data));

      let mut msg = CryptoVec::with_capacity(5 + data.len() + 1);
      msg.extend(b"\x1b[2K\r");
      msg.extend(data);
      msg.push(b'\r');
      self.conn.data(msg).await;
   }

   pub fn buf_clear(&mut self) {
      self.buffer.clear();
      self.cursor = 0;
   }
}

impl Drop for User {
   fn drop(&mut self) {
      self.handle.abort();
   }
}


pub struct Connection(russh::ChannelId, russh::server::Handle);

impl Connection {
   pub fn new(id: russh::ChannelId, handle: russh::server::Handle) -> Self {
      Self(id, handle)
   }

   pub async fn data(&self, data: CryptoVec) -> Option<()> {
      self.1.data(self.0, data).await.ok()
   }
}
