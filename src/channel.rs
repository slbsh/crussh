use std::sync::{Arc, RwLock};
use tokio::sync::{Notify, broadcast::{self, Sender}};
use std::fmt;

use crate::event::Event;

const BUFFER_SIZE: usize = 4;

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct Channel {
	pub name: Arc<str>, // TODO: make weak so it can be dropped.. 
	// or just dont have an arc, cloning the string is prob still cheeper than an atomic op
	#[serde(skip)]
	#[serde(default = "make_channel")]
	pub tx: Sender<Event>,
	// history: Vec<Event>, // TODO: one way to handle it... maybe not the best
	#[serde(deserialize_with = "perms_sorted")]
	pub perms: Arc<RwLock<Vec<PermEntry>>>,

	pub children: Arc<RwLock<Vec<Channel>>>,
	// description: Option<Arc<str>>,
}

fn make_channel() -> Sender<Event> {
	broadcast::channel(BUFFER_SIZE).0
}

fn perms_sorted<'de, D: serde::Deserializer<'de>>(d: D) 
-> Result<Arc<RwLock<Vec<PermEntry>>>, D::Error> {
	use serde::Deserialize;
	let mut perms = Vec::<PermEntry>::deserialize(d)?;
	perms.sort_unstable_by(|a, b| a.0.cmp(&b.0));
	Ok(Arc::new(RwLock::new(perms)))
}

impl fmt::Display for Channel {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		fn draw_tree(channel: &Channel, f: &mut fmt::Formatter, prefix: &str) -> fmt::Result {
			writeln!(f, "{prefix}{}\r", channel.name)?;

			let children = channel.children.read().unwrap();
			children.iter().enumerate().try_for_each(|(i, child)| {
				write!(f, "{prefix}{}─", if i == children.len() - 1 { "└" } else { "├" })?;
				draw_tree(child, f, prefix)
			})
		}

		draw_tree(self, f, "")
	}
}

pub struct SubscribedChannel {
	pub rx:     broadcast::Receiver<Event>,
	pub notify: Arc<Notify>, // we are slaves to the async
	channel:    Channel,
}

impl Channel {
	pub fn new(name: Arc<str>) -> Self {
		Self {
			name,
			tx:       broadcast::channel(BUFFER_SIZE).0,
			perms:    Arc::new(RwLock::new(Vec::new())),
			children: Arc::new(RwLock::new(Vec::new())),
		}
	}

	pub fn subscribe(self) -> SubscribedChannel {
		SubscribedChannel { 
			rx:      self.tx.subscribe(), 
			notify:  Arc::new(Notify::new()),
			channel: self,
		}
	}
}

impl SubscribedChannel {
	pub fn send(&self, event: Event) -> Result<(), broadcast::error::SendError<Event>> {
		self.tx.send(event)?;
		self.notify.notify_one();
		Ok(())
	}
}


impl std::ops::Deref for Channel {
	type Target = Sender<Event>;

	fn deref(&self) -> &Self::Target 
	{ &self.tx }
}

impl std::ops::Deref for SubscribedChannel {
	type Target = Channel;

	fn deref(&self) -> &Self::Target 
	{ &self.channel }
}


pub type PermEntry = (RestrictionKind, PermLevel);

#[derive(Clone, Debug, Ord, PartialOrd, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum RestrictionKind {
	User(Arc<str>),
	Role(Arc<str>),
	All,
}

bitflags::bitflags! {
	#[derive(Debug, Clone, Copy, Eq, PartialEq, PartialOrd, serde::Deserialize, serde::Serialize)]
	pub struct PermLevel: u8 {
		const NONE   = 0;
		const READ   = 1;
		const WRITE  = 1 << 1;
		const MANAGE = 1 << 2;
	}
}
