#[derive(Debug, serde::Deserialize)]
pub struct Config {
	pub default_channel: Box<str>,
	// pub state_file:      Box<str>,
}

impl Config {
	pub fn init() -> Self {
		let env_or = |env, or| std::env::var(env).unwrap_or(String::from(or));
		Self {
			default_channel: env_or("DEFAULT_CHANNEL", "general").into(),
			// state_file:      env_or("STATE_FILE", "state.bin").into(),
		}
	}
}
