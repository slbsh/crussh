use std::sync::{Arc, Mutex, RwLock};
use std::collections::{HashMap};

use crate::channel::{Channel, PermLevel};
use crate::user::{UserConfLock, UserConfig};
use crate::config::Config;

#[derive(Debug, serde::Deserialize)]
pub struct Server {
    pub channels:     Arc<RwLock<Vec<Channel>>>, // FIXME: double indirection
    #[serde(skip)]
    #[serde(default = "Config::init")]
    pub config:       Config,
    pub roles:        Mutex<HashMap<Arc<str>, PermLevel>>,
    pub users:        Mutex<HashMap<Arc<str>, UserConfLock>>,
    #[serde(skip)]
    pub online_users: Mutex<HashMap<Arc<str>, UserConfLock>>, // TODO: unduplicate entries
}

impl Default for Server {
    fn default() -> Self {
        let server = Self {
            channels:     Arc::new(RwLock::new(vec![Channel::new(Arc::from("general"))])),
            config:       Config::init(),
            users:        Mutex::new(HashMap::new()),
            online_users: Mutex::new(HashMap::new()),
            roles:        Mutex::new(HashMap::new()),
        };

        server.roles.lock().unwrap()
            .insert(Arc::from("admin"), PermLevel::READ|PermLevel::WRITE|PermLevel::MANAGE);
        server.users.lock().unwrap()
            .insert(Arc::from("admin"), Arc::new(Mutex::new(crate::user::UserConfig {
                hash: 0xd8acbb0fa6cac9,
                roles: vec![Arc::from("admin")],
            })));

        server
    }
}

impl Server {
    pub fn validate_pass(&self, uname: &str, pass: &str) -> Option<UserConfLock> {
        let users = self.users.lock().unwrap();
        let user = users.get(&Arc::from(uname))?;

        // rust borrow checker grrrr
        let u = (user.lock().unwrap().hash == UserConfig::hash(pass.as_bytes()))
            .then(|| Arc::clone(user)); u
    }

    // FIXME: this is cooked 💀
    // FIXME: ideally we'd return a LockGuard<Channel>, but I dont think thats possible
    // FIXME: aaaalso, it'd be nice to have a better way to only search for the channel group needed
    #[allow(clippy::type_complexity)]
    pub fn channel_from_path(path: &[&str], channels: Arc<RwLock<Vec<Channel>>>) 
        -> Result<(Arc<RwLock<Vec<Channel>>>, usize), Option<Arc<RwLock<Vec<Channel>>>>> {
        let channel = channels.clone();
        let channel = channel.read().unwrap();
        let ((index, channel),_) = channel.iter().enumerate()
            .filter_map(|c| Some((c, path.first()?)))
            .find(|((_,c),&path)| &*c.name == path)
            .ok_or_else(|| path.is_empty().then(|| channels.clone()))?;
        if path[1..].is_empty() 
            { return Ok((channels, index)); }
        Self::channel_from_path(&path[1..], channel.children.clone())
    }

    // TODO: serialize on edit/exit
}
