use buzz_client::{BuzzClient, BuzzClientConfig, BuzzIdentity};
use serde_json::json;

async fn compile_query() -> Result<(), Box<dyn std::error::Error>> {
    let identity = BuzzIdentity::parse(
        "0000000000000000000000000000000000000000000000000000000000000001",
        None,
    )?;
    let client = BuzzClient::new(BuzzClientConfig::new("https://relay.example"), identity)?;
    let filter = json!({
        "kinds": [9],
        "#h": ["00000000-0000-0000-0000-000000000000"],
        "limit": 1
    });

    let _events = client.query_values(&[filter]).await?;
    Ok(())
}

fn main() {
    let _ = compile_query;
}
