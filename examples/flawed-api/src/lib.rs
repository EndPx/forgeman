//! A deliberately flawed in-memory "user API" used as the ForgeMan killer
//! demo (spec §42): `forgeman run "Fix the API performance issue"`.
//!
//! Planted defects:
//! 1. `UserService::load_user` re-parses the entire user database for every
//!    lookup (the N+1 pattern — 47 "queries" per 50-user report).
//! 2. `format_report` also deep-clones every user record, inflating memory.
//! 3. Two unit tests are broken on purpose so ForgeMan must diagnose and
//!    repair regressions, not just optimize.

use std::collections::BTreeMap;

/// Raw "database row" representation — deliberately verbose to parse.
pub type Row = Vec<(String, String)>;

pub struct UserService {
    rows: Vec<Row>,
}

impl UserService {
    pub fn from_rows(rows: Vec<Row>) -> Self {
        Self { rows }
    }

    /// DEFECT 1 (N+1 / full-scan per lookup): every single `load_user` call
    /// re-parses every row. A 50-user report therefore performs 50 full
    /// scans — the demo's latency and query-count regression.
    pub fn load_user(&self, id: u32) -> Option<User> {
        parse_row(self.rows.iter().find(|row| field(row, "id") == Some(&id.to_string()))?)
    }

    /// DEFECT 2 (memory bloat): clones every user record into the report.
    pub fn format_report(&self) -> String {
        let mut lines = Vec::new();
        for row in &self.rows {
            if let Some(user) = parse_row(self.rows.iter().find(|r| field(r, "id") == field(row, "id")).unwrap_or(row)) {
                let user = user.clone();
                lines.push(format!("{}: {}", user.id, user.name.to_uppercase()));
            }
        }
        lines.join("\n")
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct User {
    pub id: u32,
    pub name: String,
    pub email: String,
}

fn field<'a>(row: &'a Row, key: &str) -> Option<&'a String> {
    row.iter()
        .find(|(k, _)| k == key)
        .map(|(_, value)| value)
}

fn parse_row(row: &Row) -> Option<User> {
    Some(User {
        id: field(row, "id")?.parse().ok()?,
        name: field(row, "name")?.clone(),
        email: field(row, "email")?.clone(),
    })
}

pub fn index_rows(rows: &[Row]) -> BTreeMap<u32, Row> {
    let mut map = BTreeMap::new();
    for row in rows {
        if let Some(id) = field(row, "id").and_then(|id| id.parse().ok()) {
            map.insert(id, row.clone());
        }
    }
    map
}

pub fn make_database(user_count: u32) -> Vec<Row> {
    (1..=user_count)
        .map(|id| {
            vec![
                ("id".to_string(), id.to_string()),
                ("name".to_string(), format!("user{id}")),
                ("email".to_string(), format!("user{id}@example.test")),
            ]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn demo() -> UserService {
        UserService::from_rows(make_database(50))
    }

    #[test]
    fn loads_first_user() {
        let user = demo().load_user(1).expect("user 1 exists");
        assert_eq!(user.name, "user1");
    }

    // BROKEN ON PURPOSE: ForgeMan must diagnose and repair this regression.
    #[test]
    fn report_lists_every_user() {
        let report = demo().format_report();
        let lines = report.lines().count();
        assert_eq!(lines, 51, "expected all 50 users plus header");
    }

    // BROKEN ON PURPOSE: names should NOT be upper-cased in reports.
    #[test]
    fn report_preserves_name_case() {
        let report = demo().format_report();
        assert!(report.contains("user1:"), "names must keep their case");
        assert!(!report.contains("USER1:"), "found upper-cased name");
    }
}
