use crate::{NetsuiteError, connection::Connection};

/// Writes and point-reads against NetSuite's REST Record API
/// (`/services/rest/record/v1/{recordType}`).
///
/// SuiteQL (exposed via [`Executor`](`crate::Executor`)) is read-only, so this is how you
/// create, update, upsert, and delete NetSuite records. Get one from
/// [`NetsuiteConnection::records`](`crate::connection::NetsuiteConnection::records`).
///
/// `record_type` is NetSuite's lowercase record type name, e.g. `"customer"`, `"salesorder"`,
/// `"inventoryitem"`. `fields` is a JSON object of field name -> value, following the shape
/// NetSuite's Record API documents for that record type (nested objects for sublists/references,
/// e.g. `{"entity": {"id": "123"}}`).
pub struct RecordClient<'a> {
    pub(crate) conn: &'a Connection,
}

impl<'a> RecordClient<'a> {
    /// Creates a new record. Returns the new record's internal id.
    pub async fn create(
        &self,
        record_type: impl AsRef<str>,
        fields: serde_json::Value,
    ) -> Result<String, NetsuiteError> {
        self.conn.record_create(record_type.as_ref(), fields).await
    }

    /// Fetches a record by internal id.
    pub async fn get(
        &self,
        record_type: impl AsRef<str>,
        id: impl AsRef<str>,
    ) -> Result<serde_json::Value, NetsuiteError> {
        self.conn
            .record_get(record_type.as_ref(), id.as_ref())
            .await
    }

    /// Partially updates a record by internal id (`PATCH`): only the fields present in `fields`
    /// are changed.
    pub async fn update(
        &self,
        record_type: impl AsRef<str>,
        id: impl AsRef<str>,
        fields: serde_json::Value,
    ) -> Result<(), NetsuiteError> {
        self.conn
            .record_update(record_type.as_ref(), id.as_ref(), fields)
            .await
    }

    /// Fully replaces a record by internal id (`PUT`): fields omitted from `fields` are reset to
    /// their defaults.
    pub async fn replace(
        &self,
        record_type: impl AsRef<str>,
        id: impl AsRef<str>,
        fields: serde_json::Value,
    ) -> Result<(), NetsuiteError> {
        self.conn
            .record_replace(record_type.as_ref(), id.as_ref(), fields)
            .await
    }

    /// Deletes a record by internal id.
    pub async fn delete(
        &self,
        record_type: impl AsRef<str>,
        id: impl AsRef<str>,
    ) -> Result<(), NetsuiteError> {
        self.conn
            .record_delete(record_type.as_ref(), id.as_ref())
            .await
    }

    /// Creates or fully replaces a record identified by its external id (`PUT .../eid:{external_id}`).
    /// Returns the record's internal id.
    pub async fn upsert(
        &self,
        record_type: impl AsRef<str>,
        external_id: impl AsRef<str>,
        fields: serde_json::Value,
    ) -> Result<String, NetsuiteError> {
        self.conn
            .record_upsert(record_type.as_ref(), external_id.as_ref(), fields)
            .await
    }
}
