/// Quotes a SuiteQL identifier to ensure proper casing is maintained.<br />
/// For example, `quote_ident("Customer") == "\"Customer\""`
pub fn quote_ident(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 2);
    out.push('"');
    for ch in name.chars() {
        if ch == '"' {
            out.push('"');
            out.push('"');
        } else {
            out.push(ch);
        }
    }
    out.push('"');
    out
}

/// Quotes and joins an optional schema and a table/record type name.<br />
/// For example, `qualify_table(None, "transaction") == "\"transaction\""`
pub fn qualify_table(schema: Option<&str>, table: &str) -> String {
    let mut parts = Vec::with_capacity(2);
    if let Some(s) = schema {
        parts.push(quote_ident(s));
    }
    parts.push(quote_ident(table));
    parts.join(".")
}

/// Best-effort count of positional `?` bind placeholders in a SuiteQL query, ignoring any `?`
/// that appears inside a single-quoted string literal (`''` is treated as an escaped quote).
///
/// NetSuite reports no server-side bind metadata, so this is the only way `describe()` can
/// answer `bind_count()`.
pub(crate) fn count_placeholders(sql: &str) -> i32 {
    let mut count = 0;
    let mut in_string = false;
    let mut chars = sql.chars().peekable();

    while let Some(ch) = chars.next() {
        match ch {
            '\'' => {
                if in_string && chars.peek() == Some(&'\'') {
                    chars.next();
                } else {
                    in_string = !in_string;
                }
            }
            '?' if !in_string => count += 1,
            _ => {}
        }
    }

    count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_placeholders_outside_strings_only() {
        assert_eq!(count_placeholders("SELECT * FROM t WHERE id = ?"), 1);
        assert_eq!(
            count_placeholders("SELECT * FROM t WHERE id = ? AND name = ?"),
            2
        );
        assert_eq!(
            count_placeholders("SELECT * FROM t WHERE name = 'what?' AND id = ?"),
            1
        );
        assert_eq!(
            count_placeholders("SELECT * FROM t WHERE name = 'it''s a ? test' AND id = ?"),
            1
        );
    }
}
