//! OAuth 1.0a request signing for NetSuite's Token-Based Authentication (TBA).
//!
//! NetSuite has no login step: every individual HTTP request is signed with the consumer
//! key/secret and token id/secret, so there is no session object to renew or keep alive.

use base64::Engine;
use hmac::{Hmac, Mac};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use sha2::Sha256;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

use crate::{NetsuiteError, this_errors};

/// RFC 3986 unreserved characters (`A-Za-z0-9-._~`) are the only ones OAuth 1.0a leaves unescaped.
const OAUTH_ENCODE_SET: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

pub(crate) fn percent_encode_oauth(s: &str) -> String {
    utf8_percent_encode(s, OAUTH_ENCODE_SET).to_string()
}

pub(crate) struct OAuth1Credentials<'a> {
    pub consumer_key: &'a str,
    pub consumer_secret: &'a str,
    pub token_id: &'a str,
    pub token_secret: &'a str,
    pub realm: &'a str,
}

/// Builds a signed `Authorization: OAuth ...` header for a single request.
///
/// `base_url` must be the scheme+host+path with no query string. `query_params` are the
/// request's query-string parameters (e.g. `limit`/`offset`); since NetSuite requests always
/// use a JSON body (never `application/x-www-form-urlencoded`), body fields are never part of
/// the OAuth signature base string.
pub(crate) fn build_authorization_header(
    creds: &OAuth1Credentials,
    method: &str,
    base_url: &str,
    query_params: &[(String, String)],
) -> Result<String, NetsuiteError> {
    let timestamp = this_errors!(
        "failed to read system clock",
        SystemTime::now().duration_since(UNIX_EPOCH)
    )
    .as_secs()
    .to_string();
    let nonce = Uuid::new_v4().simple().to_string();

    let mut oauth_params: Vec<(String, String)> = vec![
        (
            "oauth_consumer_key".to_string(),
            creds.consumer_key.to_string(),
        ),
        ("oauth_token".to_string(), creds.token_id.to_string()),
        (
            "oauth_signature_method".to_string(),
            "HMAC-SHA256".to_string(),
        ),
        ("oauth_timestamp".to_string(), timestamp),
        ("oauth_nonce".to_string(), nonce),
        ("oauth_version".to_string(), "1.0".to_string()),
    ];

    let mut signed_params = oauth_params.clone();
    signed_params.extend(query_params.iter().cloned());

    let mut encoded_params: Vec<(String, String)> = signed_params
        .iter()
        .map(|(k, v)| (percent_encode_oauth(k), percent_encode_oauth(v)))
        .collect();
    encoded_params.sort();

    let param_string = encoded_params
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&");

    let base_string = format!(
        "{}&{}&{}",
        method.to_uppercase(),
        percent_encode_oauth(base_url),
        percent_encode_oauth(&param_string)
    );

    let signing_key = format!(
        "{}&{}",
        percent_encode_oauth(creds.consumer_secret),
        percent_encode_oauth(creds.token_secret)
    );

    let mut mac = this_errors!(
        "failed to initialise HMAC-SHA256 signer",
        Hmac::<Sha256>::new_from_slice(signing_key.as_bytes())
    );
    mac.update(base_string.as_bytes());
    let signature = base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes());

    oauth_params.push(("oauth_signature".to_string(), signature));

    let header_params = oauth_params
        .iter()
        .map(|(k, v)| format!("{}=\"{}\"", k, percent_encode_oauth(v)))
        .collect::<Vec<_>>()
        .join(", ");

    Ok(format!(
        "OAuth realm=\"{}\", {}",
        creds.realm, header_params
    ))
}

/// Converts an account id into the lowercase, hyphenated form used in NetSuite's REST hostname
/// (e.g. `1234567_SB1` -> `1234567-sb1`).
pub(crate) fn account_id_for_host(account_id: &str) -> String {
    account_id.to_lowercase().replace('_', "-")
}

/// Converts an account id into the uppercase, underscored form NetSuite expects as the OAuth
/// `realm` (e.g. `1234567-sb1` -> `1234567_SB1`).
pub(crate) fn account_id_for_realm(account_id: &str) -> String {
    account_id.to_uppercase().replace('-', "_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_id_normalisation_round_trips() {
        assert_eq!(account_id_for_host("1234567_SB1"), "1234567-sb1");
        assert_eq!(account_id_for_realm("1234567-sb1"), "1234567_SB1");
    }

    #[test]
    fn signature_is_deterministic_shape() {
        let creds = OAuth1Credentials {
            consumer_key: "ck",
            consumer_secret: "cs",
            token_id: "tk",
            token_secret: "ts",
            realm: "1234567_SB1",
        };

        let header = build_authorization_header(
            &creds,
            "POST",
            "https://1234567-sb1.suitetalk.api.netsuite.com/services/rest/query/v1/suiteql",
            &[("limit".to_string(), "5".to_string())],
        )
        .unwrap();

        assert!(header.starts_with("OAuth realm=\"1234567_SB1\""));
        assert!(header.contains("oauth_signature_method=\"HMAC-SHA256\""));
        assert!(header.contains("oauth_consumer_key=\"ck\""));
    }
}
