use std::sync::{Arc, Mutex, RwLock};
use std::collections::{HashMap, BTreeSet};
use std::path::Path;
use tokio::sync::Mutex as AsyncMutex;
use tokio::fs::File;
use std::io::Read;

use crate::channel::{Channel, PermLevel};
use crate::user::UserConfig;

#[derive(serde::Deserialize, serde::Serialize)]
pub struct Server {
   pub root_channel: Arc<RwLock<Channel>>, // double indirection cause needs to be shared
   pub users:        HashMap<Arc<str>, Arc<Mutex<UserConfig>>>,
   #[serde(skip)]
   pub online_users: BTreeSet<Arc<str>>,
}

impl Default for Server {
   fn default() -> Self {
		let mut users = HashMap::new();
      users.insert(Arc::from("admin"), Arc::new(Mutex::new(crate::user::UserConfig {
			hash:  0xd8acbb0fa6cac9, // "admin"
			roles: vec![(Box::from("admin"), PermLevel::READ|PermLevel::WRITE|PermLevel::MANAGE)],
			.. Default::default()
		})));

      Self {
         users,
         root_channel: Arc::new(RwLock::new(Channel::new())),
         online_users: BTreeSet::new(),
      }
   }
}

impl Server {
   pub fn validate_pass(&self, uname: &str, pass: &str) -> Option<Arc<Mutex<UserConfig>>> {
      let user = self.users.get(&Arc::from(uname))?;

      // rust borrow checker grrrr
      let u = (user.lock().unwrap().hash == UserConfig::hash(pass.as_bytes()))
         .then(|| Arc::clone(user)); u
   }

	pub fn channel_from_path(&self, path: &Path) -> Option<Arc<RwLock<Channel>>> {
		fn channel_from_path(
			channels: &HashMap<Box<str>, Arc<RwLock<Channel>>>,
			mut path: std::path::Iter<'_>) 
		-> Option<Arc<RwLock<Channel>>> {
			let channel = channels.get(path.next()?.to_str()?)?;
			channel_from_path(&channel.read().unwrap().children, path)
				.unwrap_or_else(|| Arc::clone(channel))
				.into()
		}

		channel_from_path(
			&self.root_channel.read().unwrap().children,
			path.strip_prefix("/").ok()?.iter())
				.unwrap_or_else(|| Arc::clone(&self.root_channel))
				.into()
	}
}

pub struct ServerSerializer(AsyncMutex<File>, RwLock<Server>);

impl ServerSerializer {
	pub fn new(path: &str) -> Self { 
		let mut file = std::fs::OpenOptions::new()
			.read(true).write(true).create(true)
			.truncate(true).open(path)
			.unwrap_or_else(|e| panic!("Error opening {path}: {e}"));

		let mut buf = Vec::with_capacity(file.metadata().unwrap().len() as usize);
		file.read_to_end(&mut buf)
			.unwrap_or_else(|e| panic!("Error reading {path}: {e}"));

		Self(AsyncMutex::new(File::from_std(file)),
			RwLock::new(bincode::deserialize(&buf)
				.unwrap_or_default()))
	}

	pub fn read(&self) -> std::sync::RwLockReadGuard<'_, Server>
	{ self.1.read().unwrap() }

	pub fn write(&self) -> GuardMut
	{ GuardMut(self.1.write().unwrap()) }
}

pub struct GuardMut<'a>(std::sync::RwLockWriteGuard<'a, Server>);

impl<'a> std::ops::Deref for GuardMut<'a> {
	type Target = Server;
	fn deref(&self) -> &Self::Target 
	{ &self.0 }
}

impl<'a> std::ops::DerefMut for GuardMut<'a> {
	fn deref_mut(&mut self) -> &mut Self::Target
	{ &mut self.0 }
}

impl std::ops::Drop for GuardMut<'_> {
	fn drop(&mut self) {
		use tokio::io::AsyncWriteExt;
		let buf = bincode::serialize(&*self.0)
			.expect("Error serializing");

		tokio::spawn(async move {
			crate::SERVER.0.lock().await
				.write_all(&buf).await
				.expect("Error writing to file")
		});
	}
}
