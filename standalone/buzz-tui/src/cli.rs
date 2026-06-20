use nostr::Keys;

#[derive(Clone, Debug)]
pub struct BuzzCli {
    relay: String,
    private_key: Option<String>,
    public_key_hex: Option<String>,
    auth_tag: Option<String>,
}

impl BuzzCli {
    pub fn new(relay: String, private_key: Option<String>, auth_tag: Option<String>) -> Self {
        let public_key_hex = private_key
            .as_deref()
            .and_then(|private_key| Keys::parse(private_key).ok())
            .map(|keys| keys.public_key().to_hex());
        Self {
            relay,
            private_key,
            public_key_hex,
            auth_tag,
        }
    }

    pub fn relay_url(&self) -> &str {
        &self.relay
    }

    pub fn private_key(&self) -> Option<String> {
        self.private_key.clone()
    }

    pub(crate) fn public_key_hex(&self) -> Option<String> {
        self.public_key_hex.clone()
    }

    pub fn auth_tag(&self) -> Option<String> {
        self.auth_tag.clone()
    }

    pub fn set_identity(&mut self, private_key: String, auth_tag: Option<String>) {
        self.public_key_hex = Keys::parse(&private_key)
            .ok()
            .map(|keys| keys.public_key().to_hex());
        self.private_key = Some(private_key);
        self.auth_tag = auth_tag;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_key_is_cached_and_updated_with_identity() {
        let first = Keys::generate();
        let second = Keys::generate();
        let first_secret = first.secret_key().to_secret_hex();
        let second_secret = second.secret_key().to_secret_hex();
        let mut cli = BuzzCli::new("http://localhost".to_string(), Some(first_secret), None);

        assert_eq!(cli.public_key_hex(), Some(first.public_key().to_hex()));

        cli.set_identity(second_secret, None);
        assert_eq!(cli.public_key_hex(), Some(second.public_key().to_hex()));
    }
}
