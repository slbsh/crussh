use std::sync::{Arc, RwLock, Weak};
use std::collections::HashMap;
use tokio::sync::{Notify, broadcast::{self, Sender}};
use std::fmt;

use crate::event::Event;

const BUFFER_SIZE: usize = 4;

#[derive(serde::Deserialize, serde::Serialize)]
pub struct Channel {
	#[serde(skip)]
	#[serde(default = "make_channel")]
	pub tx: Sender<Event>,
	#[serde(skip)]
	#[serde(default = "make_notify")]
	pub notify: Arc<Notify>,

	#[serde(deserialize_with = "perms_sorted")]
	pub perms: Vec<PermEntry>,

	pub children: HashMap<Box<str>, Arc<RwLock<Channel>>>,

	// description: Option<Arc<str>>,
	// history: Vec<Event>, // TODO: one way to handle it... maybe not the best
}

fn make_channel() -> Sender<Event> 
	{ broadcast::channel(BUFFER_SIZE).0 }

fn make_notify() -> Arc<Notify> 
	{ Arc::new(Notify::new()) }

// sort to perserve prescedence 
// TODO: this might not actually be necessary
fn perms_sorted<'de, D: serde::Deserializer<'de>>(d: D) 
-> Result<Vec<PermEntry>, D::Error> {
	use serde::Deserialize;
	let mut perms = Vec::<PermEntry>::deserialize(d)?;
	perms.sort_unstable_by(|a, b| a.0.cmp(&b.0));
	Ok(perms)
}

impl fmt::Display for Channel {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		todo!()
		// fn draw_tree(channel: &Channel, f: &mut fmt::Formatter, prefix: &str) -> fmt::Result {
		// 	writeln!(f, "{prefix}{}\r", channel.name)?;
		//
		// 	channel.children.iter().enumerate().try_for_each(|(i, child)| {
		// 		write!(f, "{prefix}{}─", if i == channel.children.len() - 1 { "└" } else { "├" })?;
		// 		draw_tree(child.read().unwrap(), f, prefix)
		// 	})
		// }
		//
		// draw_tree(self, f, "")
	}
}

pub struct SubscribedChannel {
	pub rx:     broadcast::Receiver<Event>,
	pub tx:     Sender<Event>,
	pub notify: Arc<Notify>, // we are slaves to the async
	channel:    Weak<RwLock<Channel>>,
}

impl Channel {
	pub fn new() -> Self {
		Self {
			tx:       broadcast::channel(BUFFER_SIZE).0,
			notify:   Arc::new(Notify::new()),
			perms:    Vec::new(),
			children: HashMap::new(),
		}
	}

	pub fn subscribe(channel: &Arc<RwLock<Self>>) -> SubscribedChannel {
		let (rx, tx, notify) = {
			let channel = channel.write().unwrap();
			(channel.tx.subscribe(), channel.tx.clone(), channel.notify.clone())
		};

		SubscribedChannel { 
			rx, tx, notify,
			channel: Arc::downgrade(channel),
		}
	}
}

impl SubscribedChannel {
	pub fn send(&self, event: Event) -> Result<(), broadcast::error::SendError<Event>> {
		self.tx.send(event)?;
		self.notify.notify_waiters();
		Ok(())
	}
}


impl std::ops::Deref for SubscribedChannel {
	type Target = Weak<RwLock<Channel>>;

	fn deref(&self) -> &Self::Target 
	{ &self.channel }
}


pub type PermEntry = (RestrictionKind, PermLevel);

#[derive(Clone, Debug, Ord, PartialOrd, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum RestrictionKind {
	User(Arc<str>), // TODO: either use Weak or Box
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
