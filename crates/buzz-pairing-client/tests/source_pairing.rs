use std::sync::Arc;
use std::time::Duration;

use buzz_core::pairing::crypto;
use buzz_core::pairing::qr::decode_qr;
use buzz_core::pairing::{PairingSession, PayloadType};
use buzz_pair_relay::{run_server, Relay};
use buzz_pairing_client::{
    PairingCommand, PairingEvent, PairingSourceConfig, PairingSourceRuntime,
};
use futures_util::{SinkExt, StreamExt};
use nostr::nips::nip44;
use nostr::{Event, EventBuilder, JsonUtil, Keys, Kind, Tag, ToBech32};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

const TEST_TIMEOUT: Duration = Duration::from_secs(10);
const SUBSCRIPTION_ID: &str = "pair";

#[tokio::test]
async fn source_runtime_pairs_with_a_nip_ab_target_over_an_open_relay() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind pairing relay");
    let address = listener.local_addr().expect("pairing relay address");
    tokio::spawn(run_server(listener, Arc::new(Relay::new())));

    let source_keys = Keys::generate();
    let source_nsec = source_keys
        .secret_key()
        .to_bech32()
        .expect("encode source nsec");
    let relay_http_url = format!("http://{address}");
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let mut source = PairingSourceRuntime::new();
    source
        .start(
            PairingSourceConfig::new(relay_http_url.clone(), source_nsec.clone()),
            event_tx,
        )
        .await;

    let uri = match next_source_event(&mut event_rx).await {
        PairingEvent::Ready { uri } => uri,
        other => panic!("expected ready event, got {other:?}"),
    };
    let qr = decode_qr(uri.as_str()).expect("decode source QR");
    let pairing_relay_url = qr.relays.first().expect("QR relay").clone();
    let (mut target, offer) = PairingSession::new_target(&qr).expect("create target session");
    let target_sas = target.sas_code().expect("target SAS").to_string();

    let (mut target_socket, _) = tokio_tungstenite::connect_async(&pairing_relay_url)
        .await
        .expect("connect target");
    target_socket
        .send(Message::Text(
            serde_json::json!([
                "REQ",
                SUBSCRIPTION_ID,
                {
                    "kinds": [buzz_core::kind::KIND_PAIRING],
                    "#p": [target.pubkey().to_hex()],
                }
            ])
            .to_string()
            .into(),
        ))
        .await
        .expect("subscribe target");
    wait_for_eose(&mut target_socket).await;
    publish(&mut target_socket, &offer).await;

    match next_source_event(&mut event_rx).await {
        PairingEvent::SasReceived { code } => assert_eq!(code, target_sas),
        other => panic!("expected SAS event, got {other:?}"),
    }
    source
        .send(PairingCommand::ConfirmSas)
        .expect("confirm source SAS");

    let sas_confirm = next_target_event(&mut target_socket).await;
    assert_eq!(
        target
            .handle_sas_confirm(&sas_confirm)
            .expect("handle source confirmation"),
        target_sas
    );
    target.confirm_target_sas().expect("confirm target SAS");

    let payload_event = next_target_event(&mut target_socket).await;
    let (payload_type, payload) = target
        .handle_payload(&payload_event)
        .expect("handle source payload");
    assert_eq!(payload_type, PayloadType::Custom);
    let payload: Value = serde_json::from_str(payload.as_str()).expect("parse custom payload");
    assert_eq!(
        payload.get("relayUrl").and_then(Value::as_str),
        Some(relay_http_url.as_str())
    );
    assert_eq!(
        payload.get("pubkey").and_then(Value::as_str),
        Some(source_keys.public_key().to_hex().as_str())
    );
    assert_eq!(
        payload.get("nsec").and_then(Value::as_str),
        Some(source_nsec.as_str())
    );

    let complete = target.send_complete().expect("build complete event");
    publish(&mut target_socket, &complete).await;
    assert_eq!(
        next_source_event(&mut event_rx).await,
        PairingEvent::Complete
    );
    source.stop().await;
}

/// The target side of a `desktop-code-v1` session, driven manually over the
/// wire the same way the mobile app drives it.
struct CodeEntryTarget {
    keys: Keys,
    source: nostr::PublicKey,
}

impl CodeEntryTarget {
    fn message(&self, message: Value) -> Event {
        let encrypted = nip44::encrypt(
            self.keys.secret_key(),
            &self.source,
            message.to_string(),
            nip44::Version::V2,
        )
        .expect("encrypt target message");
        EventBuilder::new(
            Kind::Custom(buzz_core::kind::KIND_PAIRING as u16),
            encrypted,
        )
        .tags([Tag::public_key(self.source)])
        .sign_with_keys(&self.keys)
        .expect("sign target message")
    }

    fn decrypt(&self, event: &Event) -> Value {
        serde_json::from_str(
            &nip44::decrypt(self.keys.secret_key(), &event.pubkey, &event.content)
                .expect("decrypt source message"),
        )
        .expect("parse source message")
    }
}

#[tokio::test]
async fn source_runtime_releases_identity_only_after_the_target_enters_the_code() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind pairing relay");
    let address = listener.local_addr().expect("pairing relay address");
    tokio::spawn(run_server(listener, Arc::new(Relay::new())));

    let source_keys = Keys::generate();
    let source_nsec = source_keys
        .secret_key()
        .to_bech32()
        .expect("encode source nsec");
    let relay_http_url = format!("http://{address}");
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let mut source = PairingSourceRuntime::new();
    source
        .start(
            PairingSourceConfig::new(relay_http_url.clone(), source_nsec.clone()),
            event_tx,
        )
        .await;

    let uri = match next_source_event(&mut event_rx).await {
        PairingEvent::Ready { uri } => uri,
        other => panic!("expected ready event, got {other:?}"),
    };
    let qr = decode_qr(uri.as_str()).expect("decode source QR");
    let pairing_relay_url = qr.relays.first().expect("QR relay").clone();
    let target = CodeEntryTarget {
        keys: Keys::generate(),
        // The QR advertises the session's ephemeral source key, not the
        // identity key being transferred.
        source: qr.source_pubkey,
    };

    let (mut target_socket, _) = tokio_tungstenite::connect_async(&pairing_relay_url)
        .await
        .expect("connect target");
    target_socket
        .send(Message::Text(
            serde_json::json!([
                "REQ",
                SUBSCRIPTION_ID,
                {
                    "kinds": [buzz_core::kind::KIND_PAIRING],
                    "#p": [target.keys.public_key().to_hex()],
                }
            ])
            .to_string()
            .into(),
        ))
        .await
        .expect("subscribe target");
    wait_for_eose(&mut target_socket).await;

    let session_id = crypto::derive_session_id(&qr.session_secret);
    let hex = |bytes: &[u8]| bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
    let offer = target.message(json!({
        "type": "offer", "session_id": hex(&session_id), "version": 1,
        "confirmation": "desktop-code-v1"
    }));
    publish(&mut target_socket, &offer).await;

    let code = match next_source_event(&mut event_rx).await {
        PairingEvent::CodeEntryRequested { code } => code,
        other => panic!("expected code-entry event, got {other:?}"),
    };
    assert_eq!(code.len(), 6, "desktop code is six digits");
    assert!(
        code.chars().all(|c| c.is_ascii_digit()),
        "desktop code is numeric: {code}"
    );

    let challenge = next_target_event(&mut target_socket).await;
    assert_eq!(target.decrypt(&challenge), json!({"type": "desktop-code"}));

    // A manual SAS confirm must not bypass the pending code: the next event
    // the target receives is still a code rejection, not the proof.
    source
        .send(PairingCommand::ConfirmSas)
        .expect("confirm source SAS");
    let wrong_code = if code == "000000" { "000001" } else { "000000" };
    publish(
        &mut target_socket,
        &target.message(json!({
            "type": "code-submit", "code": wrong_code, "request_id": "1"
        })),
    )
    .await;
    let rejection = next_target_event(&mut target_socket).await;
    assert_eq!(
        target.decrypt(&rejection),
        json!({"type": "code-rejected", "request_id": "1", "remaining_attempts": 4})
    );
    assert!(source.is_active(), "source survives a wrong code");

    publish(
        &mut target_socket,
        &target.message(json!({
            "type": "code-submit", "code": code, "request_id": "2"
        })),
    )
    .await;
    let proof = next_target_event(&mut target_socket).await;
    let proof_message = target.decrypt(&proof);
    assert_eq!(proof_message["type"], "sas-confirm");
    let ecdh = nostr::util::generate_shared_key(target.keys.secret_key(), &qr.source_pubkey)
        .expect("derive ECDH secret");
    let (_, sas_input) = crypto::derive_sas(&ecdh, &qr.session_secret);
    let transcript_hash = crypto::derive_transcript_hash(
        &session_id,
        &qr.source_pubkey.to_bytes(),
        &target.keys.public_key().to_bytes(),
        &sas_input,
        &qr.session_secret,
    );
    assert_eq!(proof_message["transcript_hash"], hex(&transcript_hash));

    let payload_event = next_target_event(&mut target_socket).await;
    let payload_message = target.decrypt(&payload_event);
    assert_eq!(payload_message["type"], "payload");
    assert_eq!(payload_message["payload_type"], "custom");
    let payload: Value =
        serde_json::from_str(payload_message["payload"].as_str().expect("payload string"))
            .expect("parse custom payload");
    assert_eq!(
        payload.get("relayUrl").and_then(Value::as_str),
        Some(relay_http_url.as_str())
    );
    assert_eq!(
        payload.get("pubkey").and_then(Value::as_str),
        Some(source_keys.public_key().to_hex().as_str())
    );
    assert_eq!(
        payload.get("nsec").and_then(Value::as_str),
        Some(source_nsec.as_str())
    );

    publish(
        &mut target_socket,
        &target.message(json!({"type": "complete", "success": true})),
    )
    .await;
    assert_eq!(
        next_source_event(&mut event_rx).await,
        PairingEvent::Complete
    );
    source.stop().await;
}

async fn next_source_event(receiver: &mut mpsc::UnboundedReceiver<PairingEvent>) -> PairingEvent {
    tokio::time::timeout(TEST_TIMEOUT, receiver.recv())
        .await
        .expect("source event timeout")
        .expect("source event channel closed")
}

async fn wait_for_eose<S>(socket: &mut S)
where
    S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        let message = tokio::time::timeout(TEST_TIMEOUT, socket.next())
            .await
            .expect("EOSE timeout")
            .expect("target socket closed")
            .expect("target socket error");
        let Message::Text(text) = message else {
            continue;
        };
        let value: Value = serde_json::from_str(text.as_str()).expect("relay JSON");
        if value.as_array().is_some_and(|array| {
            array.first().and_then(Value::as_str) == Some("EOSE")
                && array.get(1).and_then(Value::as_str) == Some(SUBSCRIPTION_ID)
        }) {
            return;
        }
    }
}

async fn next_target_event<S>(socket: &mut S) -> Event
where
    S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        let message = tokio::time::timeout(TEST_TIMEOUT, socket.next())
            .await
            .expect("target event timeout")
            .expect("target socket closed")
            .expect("target socket error");
        let Message::Text(text) = message else {
            continue;
        };
        let value: Value = serde_json::from_str(text.as_str()).expect("relay JSON");
        let Some(array) = value.as_array() else {
            continue;
        };
        if array.first().and_then(Value::as_str) == Some("EVENT")
            && array.get(1).and_then(Value::as_str) == Some(SUBSCRIPTION_ID)
        {
            return serde_json::from_value(array.get(2).expect("event value").clone())
                .expect("parse event");
        }
    }
}

async fn publish<S>(socket: &mut S, event: &Event)
where
    S: SinkExt<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    socket
        .send(Message::Text(
            format!("[\"EVENT\",{}]", JsonUtil::as_json(event)).into(),
        ))
        .await
        .expect("publish event");
}
