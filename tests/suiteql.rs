use std::time::{Duration, Instant};

use futures_util::TryStreamExt;
use netsuite_rs::{Executor, NetsuiteConnectionOptsBuilder};
use serde_json::json;
use wiremock::matchers::{body_json, header, header_exists, method, path, query_param};
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

async fn connect_with_retry(server: &MockServer, retry_on_concurrency_limit: bool) -> netsuite_rs::NetsuiteConnection {
    let opts = NetsuiteConnectionOptsBuilder::default()
        .account_id("1234567_SB1")
        .consumer_key("ck")
        .consumer_secret("cs")
        .token_id("tk")
        .token_secret("ts")
        .host(server.uri())
        .page_size(1000)
        .pool_size(2usize)
        .retry_on_concurrency_limit(retry_on_concurrency_limit)
        .build()
        .unwrap();

    let pool = opts.connect().await.unwrap();
    pool.get().await.unwrap()
}

fn concurrency_limit_response() -> ResponseTemplate {
    ResponseTemplate::new(400).set_body_json(json!({
        "title": "Bad Request",
        "o:errorDetails": [
            {
                "detail": "Concurrent request limit exceeded. Request blocked. Verify your concurrency limits at Setup > Integration > Integration Management > Integration Governance.",
                "o:errorCode": "CONCURRENCY_LIMIT_EXCEEDED",
            }
        ],
    }))
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
async fn concurrent_pages_are_fetched_in_parallel_but_yielded_in_order() {
    let server = MockServer::start().await;

    // Page 0 (fetched by execute(), not part of the concurrent pool) - instant, no delay.
    Mock::given(method("POST"))
        .and(path("/services/rest/query/v1/suiteql"))
        .and(query_param("limit", "1"))
        .and(query_param("offset", "0"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [{"id": "1"}],
            "hasMore": true,
            "offset": 0,
            "totalResults": 5,
        })))
        .expect(1)
        .mount(&server)
        .await;

    // Offsets 1 and 2 are both slow (250ms) - if fetched sequentially that's >=500ms just for
    // these two, but with page_concurrency=3 (the default) they're launched together and should
    // overlap. Offset 3 is instant and launched in the same initial batch, and offset 4 is
    // instant but only launched once one of the first three slots frees up.
    for (offset, id, delay_ms) in [(1, "2", 250), (2, "3", 250), (3, "4", 0), (4, "5", 0)] {
        Mock::given(method("POST"))
            .and(path("/services/rest/query/v1/suiteql"))
            .and(query_param("limit", "1"))
            .and(query_param("offset", offset.to_string()))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({
                        "items": [{"id": id}],
                        "hasMore": offset < 4,
                        "offset": offset,
                        "totalResults": 5,
                    }))
                    .set_delay(Duration::from_millis(delay_ms)),
            )
            .expect(1)
            .mount(&server)
            .await;
    }

    let conn = connect(&server, 1).await;

    let query = conn.query("SELECT id FROM customer ORDER BY id").await.unwrap();
    let results = query.execute().await.unwrap();

    let start = Instant::now();
    let mut rows = results.rows();
    let mut ids = Vec::new();
    while let Some(row) = rows.try_next().await.unwrap() {
        ids.push(row.get_by_name("id").unwrap().value.to_string());
    }
    let elapsed = start.elapsed();

    // Correctness: rows must come out in page order (0,1,2,3,4) regardless of which page's
    // request actually completed first in wall-clock time.
    assert_eq!(ids, vec!["1", "2", "3", "4", "5"]);

    // Evidence of real overlap: offsets 1 and 2 (250ms each) were launched together, so total
    // time should be well under their 500ms sequential sum - comfortably bounded below that
    // while still allowing for scheduling overhead.
    assert!(
        elapsed < Duration::from_millis(450),
        "expected concurrent pages to overlap (took {elapsed:?}, sequential fetching would take >=500ms)"
    );
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
async fn describe_fills_unbound_placeholders_with_zero() {
    let server = MockServer::start().await;

    // Only the first of two placeholders is bound; describe() must fill the second itself
    // rather than sending a short `params` array, which NetSuite rejects outright. It fills
    // with `0`, not `null` - NetSuite rejects `null` as a bind value ("invalid or unsupported
    // search"), but accepts `0` against both numeric and text columns alike.
    Mock::given(method("POST"))
        .and(path("/services/rest/query/v1/suiteql"))
        .and(body_json(json!({
            "q": "SELECT id FROM customer WHERE id > ? AND name = ?",
            "params": [5, 0],
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [],
            "hasMore": false,
            "offset": 0,
            "totalResults": 0,
        })))
        .expect(1)
        .mount(&server)
        .await;

    let conn = connect(&server, 1000).await;

    let mut query = conn
        .query("SELECT id FROM customer WHERE id > ? AND name = ?")
        .await
        .unwrap();
    query.bind(5);

    let describe = query.describe().await.unwrap();
    assert_eq!(describe.bind_count(), 2);
    assert!(describe.columns().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn describe_sends_no_bind_values_already_supplied_untouched() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/services/rest/query/v1/suiteql"))
        .and(body_json(json!({
            "q": "SELECT id FROM customer WHERE id > ?",
            "params": [100],
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [{"id": "101"}],
            "hasMore": false,
            "offset": 0,
            "totalResults": 1,
        })))
        .expect(1)
        .mount(&server)
        .await;

    let conn = connect(&server, 1000).await;

    let mut query = conn.query("SELECT id FROM customer WHERE id > ?").await.unwrap();
    query.bind(100);

    let describe = query.describe().await.unwrap();
    assert_eq!(describe.bind_count(), 1);
    assert_eq!(describe.columns()[0].name, "id");
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

#[tokio::test(flavor = "current_thread")]
async fn concurrency_limit_errors_fail_immediately_by_default() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/services/rest/query/v1/suiteql"))
        .respond_with(concurrency_limit_response())
        .expect(1)
        .mount(&server)
        .await;

    let conn = connect_with_retry(&server, false).await;
    let err = conn.query("SELECT id FROM customer").await.unwrap().execute().await.unwrap_err();

    assert!(err.to_string().contains("CONCURRENCY_LIMIT_EXCEEDED"));
}

#[tokio::test(flavor = "current_thread")]
async fn concurrency_limit_errors_retry_with_backoff_when_enabled() {
    let server = MockServer::start().await;

    // First attempt fails with a concurrency-limit error; this mock only answers once, then
    // the plain success mock below (mounted after, so it's lower priority) takes over.
    Mock::given(method("POST"))
        .and(path("/services/rest/query/v1/suiteql"))
        .respond_with(concurrency_limit_response())
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/services/rest/query/v1/suiteql"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [{"id": "1"}],
            "hasMore": false,
            "offset": 0,
            "totalResults": 1,
        })))
        .expect(1)
        .mount(&server)
        .await;

    let conn = connect_with_retry(&server, true).await;

    let start = Instant::now();
    let results = conn.query("SELECT id FROM customer").await.unwrap().execute().await.unwrap();
    let elapsed = start.elapsed();

    assert_eq!(results.expected_result_length(), 1);
    // The first backoff is 500ms - confirms a retry actually happened rather than the second
    // mock being hit on the first attempt by coincidence.
    assert!(
        elapsed >= Duration::from_millis(450),
        "expected the 500ms backoff delay before the retry, got {elapsed:?}"
    );
}
