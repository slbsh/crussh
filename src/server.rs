use std::sync::{Arc, Mutex, RwLock};
use std::collections::{HashMap};
use std::path::Path;

use crate::channel::{Channel, PermLevel};
use crate::user::UserConfig;

#[derive(serde::Deserialize, serde::Serialize)]
pub struct Server {
   pub root_channel: Arc<RwLock<Channel>>,
   pub users:        Mutex<HashMap<Arc<str>, Arc<Mutex<UserConfig>>>>,
   #[serde(skip)]
   pub online_users: Mutex<HashMap<Arc<str>, Arc<Mutex<UserConfig>>>>, // TODO: unduplicate entries
}

impl Default for Server {
   fn default() -> Self {
		let mut users = HashMap::new();
      users.insert(Arc::from("admin"), Arc::new(Mutex::new(crate::user::UserConfig {
			hash: 0xd8acbb0fa6cac9, // "admin"
			roles: vec![(Box::from("admin"), PermLevel::READ|PermLevel::WRITE|PermLevel::MANAGE)],
		})));

      Self {
         root_channel: Arc::new(RwLock::new(Channel::new())),
         users:        Mutex::new(users),
         online_users: Mutex::new(HashMap::new()),
      }
   }
}

impl Server {
   pub fn validate_pass(&self, uname: &str, pass: &str) -> Option<Arc<Mutex<UserConfig>>> {
      let users = self.users.lock().unwrap();
      let user = users.get(&Arc::from(uname))?;

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

		channel_from_path(&self.root_channel.read().unwrap().children, path.iter())
			.unwrap_or_else(|| Arc::clone(&self.root_channel))
			.into()
	}


   // TODO: serialize on edit/exit
	pub fn save(&self) {
		let state = bincode::serialize(&self).unwrap();
		std::fs::write(crate::STATE_FILE, state).unwrap();
	}
}
