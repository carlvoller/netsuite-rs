use serde::Deserialize;

use crate::{
    NetsuiteError,
    auth::{
        OAuth1Credentials, account_id_for_host, account_id_for_realm, build_authorization_header,
    },
    connection::Connection,
    error, this_errors,
};

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SuiteQlResponse {
    #[serde(default)]
    pub items: Vec<serde_json::Value>,
    #[serde(default)]
    pub has_more: bool,
    #[serde(default)]
    pub total_results: i64,
}

#[derive(Deserialize, Debug, Default)]
struct NetsuiteErrorDetail {
    detail: Option<String>,
    #[serde(rename = "o:errorCode")]
    error_code: Option<String>,
}

#[derive(Deserialize, Debug, Default)]
struct NetsuiteErrorResponse {
    title: Option<String>,
    #[serde(rename = "o:errorDetails", default)]
    error_details: Vec<NetsuiteErrorDetail>,
}

fn parse_error_response(status: reqwest::StatusCode, bytes: &[u8]) -> NetsuiteError {
    if let Ok(err) = serde_json::from_slice::<NetsuiteErrorResponse>(bytes)
        && (err.title.is_some() || !err.error_details.is_empty())
    {
        let mut msg = err
            .title
            .unwrap_or_else(|| format!("NetSuite request failed with status {status}"));

        let details = err
            .error_details
            .iter()
            .map(|d| match (&d.error_code, &d.detail) {
                (Some(code), Some(detail)) => format!("{code}: {detail}"),
                (None, Some(detail)) => detail.clone(),
                (Some(code), None) => code.clone(),
                (None, None) => String::new(),
            })
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("; ");

        if !details.is_empty() {
            msg = format!("{msg}: {details}");
        }

        error!(msg)
    } else {
        let body_text = String::from_utf8_lossy(bytes);
        error!(format!(
            "NetSuite request failed with status {status}: {body_text}"
        ))
    }
}

impl Connection {
    fn base_url(&self) -> String {
        if let Some(host) = &self.opts().host {
            host.trim_end_matches('/').to_string()
        } else {
            format!(
                "https://{}.suitetalk.api.netsuite.com",
                account_id_for_host(&self.opts().account_id)
            )
        }
    }

    async fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        query: &[(String, String)],
        json_body: Option<serde_json::Value>,
        extra_headers: &[(&str, &str)],
    ) -> Result<reqwest::Response, NetsuiteError> {
        let base_url = format!("{}{}", self.base_url(), path);
        let realm = account_id_for_realm(&self.opts().account_id);

        let creds = OAuth1Credentials {
            consumer_key: &self.opts().consumer_key,
            consumer_secret: &self.opts().consumer_secret,
            token_id: &self.opts().token_id,
            token_secret: &self.opts().token_secret,
            realm: &realm,
        };

        let auth_header = build_authorization_header(&creds, method.as_str(), &base_url, query)?;

        let mut request = self
            .client
            .request(method, &base_url)
            .query(query)
            .header("Authorization", auth_header)
            .header("Content-Type", "application/json");

        for (key, value) in extra_headers {
            request = request.header(*key, *value);
        }

        if let Some(body) = json_body {
            request = request.json(&body);
        }

        Ok(this_errors!(
            "failed to send request to NetSuite",
            request.send().await
        ))
    }

    async fn parse_json_or_error<T: serde::de::DeserializeOwned>(
        &self,
        response: reqwest::Response,
    ) -> Result<T, NetsuiteError> {
        let status = response.status();
        let bytes = this_errors!(
            "failed to read NetSuite response body",
            response.bytes().await
        );

        if status.is_success() {
            Ok(this_errors!(
                "failed to parse NetSuite response as JSON",
                serde_json::from_slice::<T>(&bytes)
            ))
        } else {
            Err(parse_error_response(status, &bytes))
        }
    }

    async fn expect_no_content(&self, response: reqwest::Response) -> Result<(), NetsuiteError> {
        let status = response.status();

        if status.is_success() {
            Ok(())
        } else {
            let bytes = this_errors!(
                "failed to read NetSuite response body",
                response.bytes().await
            );
            Err(parse_error_response(status, &bytes))
        }
    }

    fn record_id_from_location(response: &reqwest::Response) -> Option<String> {
        response
            .headers()
            .get("Location")
            .and_then(|v| v.to_str().ok())
            .and_then(|loc| loc.rsplit('/').next())
            .map(|s| s.to_string())
    }

    pub(crate) async fn suiteql(
        &self,
        sql: &str,
        params: Vec<serde_json::Value>,
        limit: i64,
        offset: i64,
    ) -> Result<SuiteQlResponse, NetsuiteError> {
        let query = vec![
            ("limit".to_string(), limit.to_string()),
            ("offset".to_string(), offset.to_string()),
        ];

        let mut body = serde_json::Map::new();
        body.insert("q".to_string(), serde_json::Value::String(sql.to_string()));
        if !params.is_empty() {
            body.insert("params".to_string(), serde_json::Value::Array(params));
        }

        let response = self
            .send(
                reqwest::Method::POST,
                "/services/rest/query/v1/suiteql",
                &query,
                Some(serde_json::Value::Object(body)),
                &[("Prefer", "transient")],
            )
            .await?;

        let mut response: SuiteQlResponse = self.parse_json_or_error(response).await?;

        // NetSuite wraps every row with its own HATEOAS "links" envelope field, regardless of
        // what was selected. Strip it so it's never mistaken for a real SQL column.
        for item in &mut response.items {
            if let serde_json::Value::Object(fields) = item {
                fields.remove("links");
            }
        }

        Ok(response)
    }

    pub(crate) async fn record_create(
        &self,
        record_type: &str,
        fields: serde_json::Value,
    ) -> Result<String, NetsuiteError> {
        let path = format!("/services/rest/record/v1/{record_type}");
        let response = self
            .send(reqwest::Method::POST, &path, &[], Some(fields), &[])
            .await?;

        if response.status().is_success() {
            Self::record_id_from_location(&response).ok_or_else(|| {
                error!("NetSuite did not return a Location header for the created record")
            })
        } else {
            let status = response.status();
            let bytes = this_errors!(
                "failed to read NetSuite response body",
                response.bytes().await
            );
            Err(parse_error_response(status, &bytes))
        }
    }

    pub(crate) async fn record_get(
        &self,
        record_type: &str,
        id: &str,
    ) -> Result<serde_json::Value, NetsuiteError> {
        let path = format!("/services/rest/record/v1/{record_type}/{id}");
        let response = self
            .send(reqwest::Method::GET, &path, &[], None, &[])
            .await?;
        self.parse_json_or_error(response).await
    }

    pub(crate) async fn record_update(
        &self,
        record_type: &str,
        id: &str,
        fields: serde_json::Value,
    ) -> Result<(), NetsuiteError> {
        let path = format!("/services/rest/record/v1/{record_type}/{id}");
        let response = self
            .send(reqwest::Method::PATCH, &path, &[], Some(fields), &[])
            .await?;
        self.expect_no_content(response).await
    }

    pub(crate) async fn record_replace(
        &self,
        record_type: &str,
        id: &str,
        fields: serde_json::Value,
    ) -> Result<(), NetsuiteError> {
        let path = format!("/services/rest/record/v1/{record_type}/{id}");
        let response = self
            .send(reqwest::Method::PUT, &path, &[], Some(fields), &[])
            .await?;
        self.expect_no_content(response).await
    }

    pub(crate) async fn record_delete(
        &self,
        record_type: &str,
        id: &str,
    ) -> Result<(), NetsuiteError> {
        let path = format!("/services/rest/record/v1/{record_type}/{id}");
        let response = self
            .send(reqwest::Method::DELETE, &path, &[], None, &[])
            .await?;
        self.expect_no_content(response).await
    }

    pub(crate) async fn record_upsert(
        &self,
        record_type: &str,
        external_id: &str,
        fields: serde_json::Value,
    ) -> Result<String, NetsuiteError> {
        let path = format!("/services/rest/record/v1/{record_type}/eid:{external_id}");
        let response = self
            .send(reqwest::Method::PUT, &path, &[], Some(fields), &[])
            .await?;

        if response.status().is_success() {
            Ok(Self::record_id_from_location(&response).unwrap_or_else(|| external_id.to_string()))
        } else {
            let status = response.status();
            let bytes = this_errors!(
                "failed to read NetSuite response body",
                response.bytes().await
            );
            Err(parse_error_response(status, &bytes))
        }
    }
}
