use std::sync::Arc;

use async_lock::{Semaphore, SemaphoreGuardArc};
use derive_builder::Builder;

use crate::{NetsuiteError, this_errors};

/// Connection options for a NetSuite account, authenticated with Token-Based Authentication
/// (TBA): a consumer key/secret identifying the integration, and a token id/secret identifying
/// the user/role, both issued from NetSuite's Integration and Access Token records.
#[derive(Builder, Debug, Clone)]
pub struct NetsuiteConnectionOpts {
    #[builder(setter(into))]
    pub(crate) consumer_key: String,

    #[builder(setter(into))]
    pub(crate) consumer_secret: String,

    #[builder(setter(into))]
    pub(crate) token_id: String,

    #[builder(setter(into))]
    pub(crate) token_secret: String,

    /// Your NetSuite account id, e.g. `1234567` or `1234567_SB1` for a sandbox. Accepted in
    /// either underscore or hyphen form; it's normalised as needed for the request hostname and
    /// the OAuth `realm`.
    #[builder(setter(into))]
    pub(crate) account_id: String,

    /// Override the full base URL used for requests (scheme + host, no trailing slash).
    /// Useful for a custom domain or a NetSuite environment with a non-standard hostname.
    ///
    /// If unset, this defaults to `https://{account_id}.suitetalk.api.netsuite.com`.
    #[builder(setter(into, strip_option), default = "None")]
    pub(crate) host: Option<String>,

    /// Maximum number of NetSuite requests this pool will allow in flight at once.
    ///
    /// NetSuite TBA has no login/session step to reuse, so this simply caps concurrency to stay
    /// under NetSuite's per-account concurrency limits.
    #[builder(setter(into), default = "5")]
    pub(crate) pool_size: usize,

    /// Rows requested per SuiteQL page (used for both `fetch`/`execute` and the row stream's
    /// pagination). NetSuite caps this at 1000.
    #[builder(setter(into), default = "1000")]
    pub(crate) page_size: i64,

    /// Maximum number of SuiteQL page requests [`rows()`](`crate::query::SuiteQlQueryResult::rows`)
    /// will have in flight at once while streaming a query's remaining pages.
    ///
    /// A single page's latency is often dominated by NetSuite's own query execution time rather
    /// than by row count or how deep into the result set it is - so for a query with a large
    /// per-page cost, pagination is often bottlenecked purely by fetching pages one at a time
    /// with no overlap. Concurrency doesn't make any individual page faster, but overlapping N
    /// of them at once turns `N * page_cost` sequential wall-clock time into roughly
    /// `page_count / N * page_cost`.
    ///
    /// NetSuite enforces a single concurrency limit shared account-wide across *all* REST, SOAP,
    /// and RESTlet traffic - not just this driver - and it can be as low as 5 concurrent
    /// requests on an entry-tier account with no SuiteCloud Plus licenses. The default of `3`
    /// leaves headroom under that floor; raise it if your account has a higher limit (Setup >
    /// Integration > Integration Management > Integration Governance), or set it to `1` to
    /// restore fully sequential fetching. Exceeding your account's limit surfaces as an error
    /// from the offending page request, same as any other failed page - unless
    /// `retry_on_concurrency_limit` is enabled, in which case that one request is retried
    /// instead.
    #[builder(setter(into), default = "3")]
    pub(crate) page_concurrency: usize,

    /// When `true`, a request that fails with NetSuite's `CONCURRENCY_LIMIT_EXCEEDED` error is
    /// retried with exponential backoff (500ms, 1s, 2s, 4s, 8s - up to 5 attempts) instead of
    /// failing immediately.
    ///
    /// NetSuite's concurrency limit is a single pool shared account-wide across *all* REST,
    /// SOAP, and RESTlet traffic, not just this driver's own `pool_size`/`page_concurrency` - so
    /// even a `page_concurrency` comfortably under your account's configured limit can
    /// occasionally collide with other concurrent activity on the account (another integration,
    /// a scheduled script, a user in the UI) and get rejected. Without this, that one rejected
    /// request fails the whole in-progress operation (e.g. aborts a `rows()` stream entirely,
    /// discarding whatever had already been fetched); with it, only that one request is delayed
    /// and retried.
    ///
    /// Defaults to `false`, since this changes error and timing behavior for anyone already
    /// handling `CONCURRENCY_LIMIT_EXCEEDED` themselves - opt in explicitly.
    #[builder(setter(into), default = "false")]
    pub(crate) retry_on_concurrency_limit: bool,
}

impl NetsuiteConnectionOpts {
    // TODO: Consider removing async? I feel like maybe keeping async
    // in case I intend to implement some form of ping here in the future.
    /// Builds a [`NetsuitePool`]. This never makes a network call.
    pub async fn connect(self) -> Result<NetsuitePool, NetsuiteError> {
        let client = this_errors!(
            "failed to build http client",
            reqwest::Client::builder().gzip(true).referer(false).build()
        );

        let pool_size = self.pool_size;

        Ok(NetsuitePool {
            conn: Connection {
                client,
                opts: Arc::new(self),
            },
            semaphore: Arc::new(Semaphore::new(pool_size)),
        })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Connection {
    pub(crate) client: reqwest::Client,
    pub(crate) opts: Arc<NetsuiteConnectionOpts>,
}

impl Connection {
    pub(crate) fn opts(&self) -> &NetsuiteConnectionOpts {
        &self.opts
    }
}

/// A pool that caps how many NetSuite requests are in flight at once.
#[derive(Clone)]
pub struct NetsuitePool {
    conn: Connection,
    semaphore: Arc<Semaphore>,
}

impl NetsuitePool {
    pub async fn get(&self) -> Result<NetsuiteConnection, NetsuiteError> {
        let permit = self.semaphore.acquire_arc().await;

        Ok(NetsuiteConnection {
            conn: self.conn.clone(),
            _permit: permit,
        })
    }
}

#[derive(Debug)]
pub struct NetsuiteConnection {
    pub(crate) conn: Connection,
    _permit: SemaphoreGuardArc,
}

impl NetsuiteConnection {
    /// Returns a [`RecordClient`](`crate::record::RecordClient`) for creating, updating,
    /// upserting, and deleting records via NetSuite's REST Record API.
    pub fn records(&self) -> crate::record::RecordClient<'_> {
        crate::record::RecordClient { conn: &self.conn }
    }
}
