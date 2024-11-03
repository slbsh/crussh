#[derive(Debug, serde::Deserialize)]
pub struct Config {
    pub default_channel: Box<str>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            default_channel: Box::from("general"),
        }
    }
}

impl Config {
    pub fn init() -> Self {
        let env_or = |env, or| std::env::var(env).unwrap_or(String::from(or));
        Self {
            default_channel: env_or("DEFAULT_CHANNEL", "general").into(),
        }
    }
}
