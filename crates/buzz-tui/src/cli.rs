#[derive(Clone, Debug)]
pub struct BuzzCli {
    relay: String,
    private_key: Option<String>,
    auth_tag: Option<String>,
}

impl BuzzCli {
    pub fn new(relay: String, private_key: Option<String>, auth_tag: Option<String>) -> Self {
        Self {
            relay,
            private_key,
            auth_tag,
        }
    }

    pub fn relay_url(&self) -> &str {
        &self.relay
    }

    pub fn private_key(&self) -> Option<String> {
        self.private_key.clone()
    }

    pub fn auth_tag(&self) -> Option<String> {
        self.auth_tag.clone()
    }
}
