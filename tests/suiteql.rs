use futures_util::TryStreamExt;
use netsuite_rs::{Executor, NetsuiteConnectionOptsBuilder};
use serde_json::json;
use wiremock::matchers::{header, header_exists, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn connect(server: &MockServer, page_size: i64) -> netsuite_rs::NetsuiteConnection {
    let opts = NetsuiteConnectionOptsBuilder::default()
        .account_id("1234567_SB1")
        .consumer_key("ck")
        .consumer_secret("cs")
        .token_id("tk")
        .token_secret("ts")
        .host(server.uri())
        .page_size(page_size)
        .pool_size(2usize)
        .build()
        .unwrap();

    let pool = opts.connect().await.unwrap();
    pool.get().await.unwrap()
}

#[tokio::test(flavor = "current_thread")]
async fn streams_rows_across_pages_in_order() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/services/rest/query/v1/suiteql"))
        .and(query_param("limit", "2"))
        .and(query_param("offset", "0"))
        .and(header("Prefer", "transient"))
        .and(header_exists("Authorization"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [
                {"id": "1", "companyname": "Acme"},
                {"id": "2", "companyname": "Globex"},
            ],
            "hasMore": true,
            "offset": 0,
            "totalResults": 3,
        })))
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/services/rest/query/v1/suiteql"))
        .and(query_param("limit", "2"))
        .and(query_param("offset", "2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [
                {"id": "3", "companyname": "Initech"},
            ],
            "hasMore": false,
            "offset": 2,
            "totalResults": 3,
        })))
        .expect(1)
        .mount(&server)
        .await;

    let conn = connect(&server, 2).await;

    let mut query = conn
        .query("SELECT id, companyname FROM customer WHERE id > ?")
        .await
        .unwrap();
    query.bind(0);

    let results = query.execute().await.unwrap();
    assert_eq!(results.expected_result_length(), 3);

    let columns = results.columns();
    assert_eq!(columns.len(), 2);
    assert_eq!(columns[0].name, "id");
    assert_eq!(columns[1].name, "companyname");

    let mut rows = results.rows();
    let mut ids = Vec::new();
    while let Some(row) = rows.try_next().await.unwrap() {
        let id = row.get_by_name("id").unwrap();
        ids.push(id.value.to_string());
    }

    assert_eq!(ids, vec!["1", "2", "3"]);
}

#[tokio::test(flavor = "current_thread")]
async fn strips_netsuites_hateoas_links_envelope_field() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/services/rest/query/v1/suiteql"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [
                {
                    "id": "1",
                    "companyname": "Acme",
                    "links": [{"rel": "self", "href": "https://example.com/record/v1/customer/1"}],
                },
            ],
            "hasMore": false,
            "offset": 0,
            "totalResults": 1,
        })))
        .mount(&server)
        .await;

    let conn = connect(&server, 1000).await;

    let results = conn
        .query("SELECT id, companyname FROM customer")
        .await
        .unwrap()
        .execute()
        .await
        .unwrap();

    let columns = results.columns();
    assert_eq!(columns.len(), 2, "the injected `links` field must not be inferred as a column");
    assert!(columns.iter().all(|c| c.name != "links"));

    let mut rows = results.rows();
    let row = rows.try_next().await.unwrap().unwrap();
    assert!(row.get_by_name("links").is_err());
}

#[tokio::test(flavor = "current_thread")]
async fn fetch_all_buffers_every_page() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/services/rest/query/v1/suiteql"))
        .and(query_param("offset", "0"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [{"id": "1"}],
            "hasMore": true,
            "offset": 0,
            "totalResults": 2,
        })))
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/services/rest/query/v1/suiteql"))
        .and(query_param("offset", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [{"id": "2"}],
            "hasMore": false,
            "offset": 1,
            "totalResults": 2,
        })))
        .mount(&server)
        .await;

    let conn = connect(&server, 1).await;
    let rows = conn.fetch_all("SELECT id FROM customer").await.unwrap();

    assert_eq!(rows.len(), 2);
}

#[tokio::test(flavor = "current_thread")]
async fn describe_uses_a_single_row_limit_and_counts_placeholders() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/services/rest/query/v1/suiteql"))
        .and(query_param("limit", "1"))
        .and(query_param("offset", "0"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [{"id": "1", "isactive": true}],
            "hasMore": true,
            "offset": 0,
            "totalResults": 100,
        })))
        .mount(&server)
        .await;

    let conn = connect(&server, 1000).await;

    let describe = conn
        .query("SELECT id, isactive FROM customer WHERE id > ? AND name = 'a?b'")
        .await
        .unwrap()
        .describe()
        .await
        .unwrap();

    assert_eq!(describe.bind_count(), 1);
    assert!(!describe.is_dml());
    assert!(describe.is_dql());

    let columns = describe.columns();
    assert_eq!(columns[0].name, "id");
    assert_eq!(columns[1].name, "isactive");
    assert_eq!(columns[1].col_type, netsuite_rs::ColumnType::Boolean);
}

#[tokio::test(flavor = "current_thread")]
async fn record_create_extracts_id_from_location_header() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/services/rest/record/v1/customer"))
        .respond_with(
            ResponseTemplate::new(204)
                .insert_header("Location", "https://example.com/record/v1/customer/42"),
        )
        .mount(&server)
        .await;

    let conn = connect(&server, 1000).await;
    let id = conn
        .records()
        .create("customer", json!({"companyName": "Acme"}))
        .await
        .unwrap();

    assert_eq!(id, "42");
}

#[tokio::test(flavor = "current_thread")]
async fn record_update_and_delete_hit_expected_paths() {
    let server = MockServer::start().await;

    Mock::given(method("PATCH"))
        .and(path("/services/rest/record/v1/customer/42"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("DELETE"))
        .and(path("/services/rest/record/v1/customer/42"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;

    let conn = connect(&server, 1000).await;
    conn.records()
        .update("customer", "42", json!({"companyName": "Renamed"}))
        .await
        .unwrap();
    conn.records().delete("customer", "42").await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn record_upsert_puts_to_external_id_path() {
    let server = MockServer::start().await;

    Mock::given(method("PUT"))
        .and(path("/services/rest/record/v1/customer/eid:ext-123"))
        .respond_with(
            ResponseTemplate::new(204)
                .insert_header("Location", "https://example.com/record/v1/customer/99"),
        )
        .mount(&server)
        .await;

    let conn = connect(&server, 1000).await;
    let id = conn
        .records()
        .upsert("customer", "ext-123", json!({"companyName": "Acme"}))
        .await
        .unwrap();

    assert_eq!(id, "99");
}

#[tokio::test(flavor = "current_thread")]
async fn error_responses_surface_netsuite_error_details() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/services/rest/query/v1/suiteql"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "title": "Invalid search query.",
            "o:errorDetails": [
                {"detail": "Search error occurred: Field 'foo' not found.", "o:errorCode": "USER_ERROR"}
            ],
        })))
        .mount(&server)
        .await;

    let conn = connect(&server, 1000).await;
    let err = conn
        .query("SELECT foo FROM bar")
        .await
        .unwrap()
        .execute()
        .await
        .unwrap_err();

    let message = err.to_string();
    assert!(message.contains("Invalid search query."));
    assert!(message.contains("Field 'foo' not found."));
}
