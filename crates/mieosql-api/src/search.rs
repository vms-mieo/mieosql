//! Database-wide search: find objects, columns and (optionally) definitions
//! matching a pattern across a whole connection.
//!
//! Part of the v1 feature pack. The DTOs and method signatures below are a
//! frozen contract: implementations fill the bodies, but callers (Tauri
//! commands, CLI, tests) rely on these exact shapes.

use regex::RegexBuilder;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use mieosql_core::{
    ColumnDef, Engine, ErrorCode, MieoError, ObjectKind, ObjectRef, Scope,
};
use mieosql_engine::SessionInfo;

use crate::dto::ApiResult;
use crate::service::Api;

/// How the pattern is matched against names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SearchMatchMode {
    /// `pattern` appears anywhere in the name.
    Contains,
    /// Name starts with `pattern`.
    Prefix,
    /// `pattern` matches a whole word (identifier boundaries).
    WholeWord,
    /// `pattern` is a regular expression (Rust regex syntax).
    Regex,
}

/// Which part of an object matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SearchHitField {
    /// The object name itself.
    Name,
    /// A column name.
    Column,
    /// The object definition (view SQL / DDL) when requested.
    Definition,
}

/// What to search for, and where.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SearchRequest {
    pub pattern: String,
    /// Case-sensitive matching (default: case-insensitive).
    #[serde(default)]
    pub case_sensitive: bool,
    pub mode: SearchMatchMode,
    /// Object kinds to scan; empty = tables + views.
    #[serde(default)]
    pub kinds: Vec<ObjectKind>,
    /// Databases to search; empty = every database visible to the session.
    #[serde(default)]
    pub databases: Vec<String>,
    /// Schemas to search; empty = every schema of the selected databases.
    #[serde(default)]
    pub schemas: Vec<String>,
    /// Also match column names.
    #[serde(default = "default_true")]
    pub include_columns: bool,
    /// Also match view/table definitions (engine-dependent, best effort).
    #[serde(default)]
    pub include_definitions: bool,
    /// Cap on returned hits (1..=5000).
    #[serde(default = "default_limit")]
    pub limit: u32,
}

fn default_true() -> bool {
    true
}

fn default_limit() -> u32 {
    500
}

/// One matched object / column.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SearchHit {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub database: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub schema: Option<String>,
    /// Table / view / function name.
    pub object: String,
    pub kind: ObjectKind,
    /// Column name when the hit came from a column.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub column: Option<String>,
    /// Native column type, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub column_type: Option<String>,
    /// A short excerpt around the match for definition hits.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub excerpt: Option<String>,
    pub matched_in: SearchHitField,
}

/// Result set of a database search.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SearchResponse {
    pub hits: Vec<SearchHit>,
    /// Objects (tables/views) inspected.
    #[ts(type = "number")]
    pub scanned_objects: u64,
    /// True when `limit` cut the hit list short.
    pub truncated: bool,
}

// ---------------------------------------------------------------------------
// Implementation
// ---------------------------------------------------------------------------

/// Hard cap on the number of returned hits (mirrors the DTO doc `1..=5000`).
const MAX_HITS: u32 = 5000;
/// Longest excerpt kept for a definition hit, in characters.
const MAX_EXCERPT_CHARS: usize = 200;

/// Compiled pattern matcher. Pure logic with no database access, so every
/// match mode can be unit tested directly.
#[derive(Debug)]
struct PatternMatcher {
    pattern: String,
    lower_pattern: String,
    mode: SearchMatchMode,
    case_sensitive: bool,
    regex: Option<regex::Regex>,
}

impl PatternMatcher {
    /// Validate and compile a request pattern.
    fn build(request: &SearchRequest) -> Result<Self, MieoError> {
        if request.pattern.trim().is_empty() {
            return Err(
                MieoError::new(ErrorCode::Data, "search pattern is empty")
                    .with_hint("provide a non-empty pattern"),
            );
        }
        let regex = if request.mode == SearchMatchMode::Regex {
            Some(
                RegexBuilder::new(&request.pattern)
                    .case_insensitive(!request.case_sensitive)
                    .build()
                    .map_err(|err| {
                        MieoError::new(ErrorCode::Syntax, "invalid pattern")
                            .with_hint(format!("regex error: {err}"))
                    })?,
            )
        } else {
            None
        };
        Ok(Self {
            pattern: request.pattern.clone(),
            lower_pattern: request.pattern.to_lowercase(),
            mode: request.mode,
            case_sensitive: request.case_sensitive,
            regex,
        })
    }

    /// Does `text` match the pattern under the request's mode + case rule?
    fn matches(&self, text: &str) -> bool {
        match self.mode {
            SearchMatchMode::Contains => {
                if self.case_sensitive {
                    text.contains(&self.pattern)
                } else {
                    text.to_lowercase().contains(&self.lower_pattern)
                }
            }
            SearchMatchMode::Prefix => {
                if self.case_sensitive {
                    text.starts_with(&self.pattern)
                } else {
                    text.to_lowercase().starts_with(&self.lower_pattern)
                }
            }
            SearchMatchMode::WholeWord => self.find_whole_word(text).is_some(),
            SearchMatchMode::Regex => self
                .regex
                .as_ref()
                .is_some_and(|regex| regex.is_match(text)),
        }
    }

    /// Byte range of the first match in `text`, used to build excerpts. The
    /// offsets always fall on char boundaries of `text`.
    fn find(&self, text: &str) -> Option<(usize, usize)> {
        match self.mode {
            SearchMatchMode::Contains if self.case_sensitive => text
                .find(&self.pattern)
                .map(|i| (i, i + self.pattern.len())),
            SearchMatchMode::Contains => self.find_ci(text, false),
            SearchMatchMode::Prefix if self.case_sensitive => text
                .starts_with(&self.pattern)
                .then(|| (0, self.pattern.len())),
            SearchMatchMode::Prefix => self.find_ci(text, true),
            SearchMatchMode::WholeWord => self.find_whole_word(text),
            SearchMatchMode::Regex => self
                .regex
                .as_ref()
                .and_then(|regex| regex.find(text))
                .map(|m| (m.start(), m.end())),
        }
    }

    /// Case-insensitive occurrence search that keeps byte offsets valid for
    /// the original text. Mapping indices through `to_lowercase` is unsafe
    /// when a char's lowercase form changes byte/char length, so windows are
    /// compared char by char instead.
    fn find_ci(&self, text: &str, anchored: bool) -> Option<(usize, usize)> {
        let needle_chars = self.lower_pattern.chars().count();
        if needle_chars == 0 {
            return Some((0, 0));
        }
        if anchored {
            let candidate: String = text.chars().take(needle_chars).collect();
            if candidate.chars().count() == needle_chars
                && candidate.to_lowercase() == self.lower_pattern
            {
                return Some((0, candidate.len()));
            }
            return None;
        }
        for (start, _) in text.char_indices() {
            let candidate: String = text[start..].chars().take(needle_chars).collect();
            if candidate.chars().count() < needle_chars {
                break;
            }
            if candidate.to_lowercase() == self.lower_pattern {
                return Some((start, start + candidate.len()));
            }
        }
        None
    }

    /// First whole-word occurrence: a word boundary is a transition between
    /// `[A-Za-z0-9_]` and any other char (or a string end). Scanning every
    /// char start also catches occurrences a non-overlapping substring search
    /// would skip, e.g. `a-a-` inside `a-a-a-`.
    fn find_whole_word(&self, text: &str) -> Option<(usize, usize)> {
        let needle_chars = self.pattern.chars().count();
        if needle_chars == 0 {
            return None;
        }
        for (start, _) in text.char_indices() {
            let candidate: String = text[start..].chars().take(needle_chars).collect();
            if candidate.chars().count() < needle_chars {
                break;
            }
            let same = if self.case_sensitive {
                candidate == self.pattern
            } else {
                candidate.to_lowercase() == self.lower_pattern
            };
            if same {
                let end = start + candidate.len();
                if is_word_boundary(text, start, end) {
                    return Some((start, end));
                }
            }
        }
        None
    }
}

/// `[A-Za-z0-9_]` — identifier characters for whole-word matching.
fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn is_word_boundary(text: &str, start: usize, end: usize) -> bool {
    let left = text[..start]
        .chars()
        .next_back()
        .is_none_or(|c| !is_word_char(c));
    let right = text[end..]
        .chars()
        .next()
        .is_none_or(|c| !is_word_char(c));
    left && right
}

/// Build the excerpt shown for a definition hit: the first line containing a
/// match, trimmed and capped at [`MAX_EXCERPT_CHARS`] characters.
fn excerpt_for(text: &str, matcher: &PatternMatcher) -> String {
    if let Some(line) = text.lines().find(|line| matcher.matches(line)) {
        return truncate_chars(line.trim(), MAX_EXCERPT_CHARS);
    }
    // The match spans a line break (e.g. a regex with `(?s)`): show the line
    // where the raw match starts.
    if let Some((start, end)) = matcher.find(text) {
        let line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
        let line_end = text[end..].find('\n').map_or(text.len(), |i| end + i);
        return truncate_chars(text[line_start..line_end].trim(), MAX_EXCERPT_CHARS);
    }
    truncate_chars(text.trim(), MAX_EXCERPT_CHARS)
}

/// Char-safe cut to `max` characters; a truncated string ends with `…` so the
/// result never exceeds `max` chars.
fn truncate_chars(text: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut chars = text.chars();
    for _ in 0..max {
        match chars.next() {
            Some(c) => out.push(c),
            None => return out,
        }
    }
    if chars.next().is_some() {
        out.pop();
        out.push('…');
    }
    out
}

/// Kinds actually scanned: the requested ones, or tables + views by default.
fn effective_kinds(requested: &[ObjectKind]) -> Vec<ObjectKind> {
    if requested.is_empty() {
        vec![ObjectKind::Table, ObjectKind::View]
    } else {
        requested.to_vec()
    }
}

/// Returned-hit cap, clamped to the documented `1..=5000` range.
fn effective_limit(limit: u32) -> usize {
    limit.clamp(1, MAX_HITS) as usize
}

/// Can `object_ddl` produce a definition worth searching for this kind?
fn has_definition(kind: ObjectKind) -> bool {
    matches!(
        kind,
        ObjectKind::Table | ObjectKind::View | ObjectKind::MaterializedView
    )
}

fn name_hit(object: &ObjectRef, scope: &Scope) -> SearchHit {
    SearchHit {
        database: object.database.clone().or_else(|| scope.database.clone()),
        schema: object.schema.clone().or_else(|| scope.schema.clone()),
        object: object.name.clone(),
        kind: object.kind,
        column: None,
        column_type: None,
        excerpt: None,
        matched_in: SearchHitField::Name,
    }
}

fn column_hit(object: &ObjectRef, scope: &Scope, column: &ColumnDef) -> SearchHit {
    SearchHit {
        database: object.database.clone().or_else(|| scope.database.clone()),
        schema: object.schema.clone().or_else(|| scope.schema.clone()),
        object: object.name.clone(),
        kind: object.kind,
        column: Some(column.name.clone()),
        column_type: Some(column.native_type.clone()),
        excerpt: None,
        matched_in: SearchHitField::Column,
    }
}

fn definition_hit(object: &ObjectRef, scope: &Scope, excerpt: String) -> SearchHit {
    SearchHit {
        database: object.database.clone().or_else(|| scope.database.clone()),
        schema: object.schema.clone().or_else(|| scope.schema.clone()),
        object: object.name.clone(),
        kind: object.kind,
        column: None,
        column_type: None,
        excerpt: Some(excerpt),
        matched_in: SearchHitField::Definition,
    }
}

/// Push `hit` unless `limit` is reached; returns `false` when the hit was
/// dropped (the caller then reports `truncated` and stops scanning).
fn try_push(hits: &mut Vec<SearchHit>, limit: usize, hit: SearchHit) -> bool {
    if hits.len() >= limit {
        false
    } else {
        hits.push(hit);
        true
    }
}

impl Api {
    /// Search the whole connection for objects/columns matching `request`.
    pub async fn search_database(
        &self,
        session_id: String,
        request: SearchRequest,
    ) -> ApiResult<SearchResponse> {
        let matcher = PatternMatcher::build(&request)?;
        let limit = effective_limit(request.limit);
        let kinds = effective_kinds(&request.kinds);

        let info = self.session_info(session_id.clone()).await?;
        let scopes = self.search_scopes(&session_id, &info, &request).await?;

        let mut hits: Vec<SearchHit> = Vec::new();
        let mut scanned_objects: u64 = 0;
        let mut truncated = false;

        // NOTE (v1): the scan is sequential — one catalog round-trip per
        // kind/object (columns + DDL on demand). Good enough for the catalogs
        // this targets; a batched catalog query can come in a later pass.

        'scan: for (scope_index, scope) in scopes.iter().enumerate() {
            for (kind_index, kind) in kinds.iter().enumerate() {
                let objects = self
                    .list_objects(session_id.clone(), scope.clone(), Some(*kind), None)
                    .await?;
                let mut objects = objects.into_iter().peekable();
                while let Some(object) = objects.next() {
                    // Counted up to the early-stop point per the contract.
                    scanned_objects += 1;

                    // Pushing the hit that would exceed `limit` sets
                    // `truncated` and stops the whole scan.
                    if matcher.matches(&object.name)
                        && !try_push(&mut hits, limit, name_hit(&object, scope))
                    {
                        truncated = true;
                        break 'scan;
                    }

                    // Column matching is independent of the object name: a
                    // table whose name does not match can still produce
                    // column hits, and vice versa.
                    if request.include_columns {
                        let columns = self
                            .table_columns(session_id.clone(), scope.clone(), object.name.clone())
                            .await?;
                        for column in columns {
                            if matcher.matches(&column.name)
                                && !try_push(
                                    &mut hits,
                                    limit,
                                    column_hit(&object, scope, &column),
                                )
                            {
                                truncated = true;
                                break 'scan;
                            }
                        }
                    }

                    if request.include_definitions && has_definition(object.kind) {
                        let ddl = self
                            .object_ddl(
                                session_id.clone(),
                                scope.clone(),
                                object.kind,
                                object.name.clone(),
                            )
                            .await?;
                        if let Some(ddl) = ddl {
                            if matcher.matches(&ddl) {
                                let excerpt = excerpt_for(&ddl, &matcher);
                                if !try_push(
                                    &mut hits,
                                    limit,
                                    definition_hit(&object, scope, excerpt),
                                ) {
                                    truncated = true;
                                    break 'scan;
                                }
                            }
                        }
                    }

                    // Hit cap reached exactly: stop before doing more catalog
                    // I/O. If any object/listing is left unscanned, results
                    // may be incomplete, so report `truncated`.
                    if hits.len() >= limit {
                        let more_objects = objects.peek().is_some();
                        let more_listings =
                            scope_index + 1 < scopes.len() || kind_index + 1 < kinds.len();
                        truncated = more_objects || more_listings;
                        break 'scan;
                    }
                }
            }
        }

        Ok(SearchResponse {
            hits,
            scanned_objects,
            truncated,
        })
    }

    /// Resolve which scopes to scan, per engine.
    ///
    /// - MySQL/MariaDB: every non-system database (or `request.databases`).
    /// - PostgreSQL: the session database (or `request.databases`) × every
    ///   non-system schema (or `request.schemas`).
    /// - SQLite: `{}` (or the first requested database, e.g. `main`/`temp`).
    async fn search_scopes(
        &self,
        session_id: &str,
        info: &SessionInfo,
        request: &SearchRequest,
    ) -> ApiResult<Vec<Scope>> {
        match info.engine {
            Engine::MySql | Engine::MariaDb => {
                let databases: Vec<String> = if request.databases.is_empty() {
                    self.list_databases(session_id.to_string())
                        .await?
                        .into_iter()
                        .filter(|db| !db.is_system)
                        .map(|db| db.name)
                        .collect()
                } else {
                    request.databases.clone()
                };
                Ok(databases.into_iter().map(|db| Scope::db(db)).collect())
            }
            Engine::Postgres => {
                // A Postgres connection is pinned to one database, but honor
                // explicit database names in the request.
                let databases: Vec<Option<String>> = if request.databases.is_empty() {
                    vec![info.database.clone()]
                } else {
                    request.databases.iter().cloned().map(Some).collect()
                };
                let mut scopes = Vec::new();
                for database in databases {
                    let schemas: Vec<String> = if request.schemas.is_empty() {
                        self.list_schemas(session_id.to_string(), database.clone())
                            .await?
                            .into_iter()
                            .filter(|schema| !schema.is_system)
                            .map(|schema| schema.name)
                            .collect()
                    } else {
                        request.schemas.clone()
                    };
                    for schema in schemas {
                        scopes.push(Scope {
                            database: database.clone(),
                            schema: Some(schema),
                        });
                    }
                }
                Ok(scopes)
            }
            Engine::Sqlite => Ok(vec![Scope {
                database: request.databases.first().cloned(),
                schema: None,
            }]),
            other => Err(MieoError::unsupported(format!(
                "database search is not supported for {}",
                other.label()
            ))
            .into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(pattern: &str, mode: SearchMatchMode, case_sensitive: bool) -> SearchRequest {
        SearchRequest {
            pattern: pattern.to_string(),
            case_sensitive,
            mode,
            kinds: Vec::new(),
            databases: Vec::new(),
            schemas: Vec::new(),
            include_columns: true,
            include_definitions: false,
            limit: 500,
        }
    }

    fn matcher(pattern: &str, mode: SearchMatchMode, case_sensitive: bool) -> PatternMatcher {
        PatternMatcher::build(&request(pattern, mode, case_sensitive)).expect("pattern builds")
    }

    #[test]
    fn contains_matches_case_insensitively_by_default() {
        let m = matcher("user", SearchMatchMode::Contains, false);
        assert!(m.matches("users"));
        assert!(m.matches("USER_LOG"));
        assert!(!m.matches("orders"));
    }

    #[test]
    fn contains_honors_case_sensitivity() {
        let m = matcher("User", SearchMatchMode::Contains, true);
        assert!(m.matches("UserLog"));
        assert!(!m.matches("user_log"));
    }

    #[test]
    fn prefix_matches_only_the_start() {
        let m = matcher("user", SearchMatchMode::Prefix, false);
        assert!(m.matches("users"));
        assert!(m.matches("UserLog"));
        assert!(!m.matches("audit_user"));
    }

    #[test]
    fn whole_word_respects_identifier_boundaries() {
        let m = matcher("user", SearchMatchMode::WholeWord, true);
        assert!(m.matches("user"));
        assert!(m.matches("user.email")); // '.' is a boundary
        assert!(m.matches("the user table")); // space is a boundary
        assert!(!m.matches("user_id")); // '_' is a word char
        assert!(!m.matches("superuser"));
        assert!(!m.matches("users"));
    }

    #[test]
    fn whole_word_is_case_insensitive_when_asked() {
        let m = matcher("USER", SearchMatchMode::WholeWord, false);
        assert!(m.matches("some User table"));
        assert!(!m.matches("some user_id table"));
    }

    #[test]
    fn whole_word_catches_overlapping_occurrences() {
        // A non-overlapping substring scan would find `a-a-` at 0, fail the
        // right boundary ('a' follows) and miss the valid occurrence at 2.
        let m = matcher("a-a-", SearchMatchMode::WholeWord, true);
        assert!(m.matches("a-a-a-"));
        assert_eq!(m.find("a-a-a-"), Some((2, 6)));
    }

    #[test]
    fn boundaries_are_identifier_chars() {
        assert!(!is_word_boundary("user_id", 0, 4)); // '_' follows
        assert!(!is_word_boundary("xuser", 1, 5)); // 'x' precedes
        assert!(is_word_boundary("user", 0, 4));
        assert!(is_word_boundary("a user!", 2, 6));
    }

    #[test]
    fn regex_mode_honors_case_sensitivity() {
        let ci = matcher("^order_[0-9]+$", SearchMatchMode::Regex, false);
        assert!(ci.matches("ORDER_42"));
        assert!(!ci.matches("order_x"));

        let cs = matcher("^order_[0-9]+$", SearchMatchMode::Regex, true);
        assert!(!cs.matches("ORDER_42"));
        assert!(cs.matches("order_42"));
    }

    #[test]
    fn invalid_regex_is_a_syntax_error() {
        let err = PatternMatcher::build(&request("(unclosed", SearchMatchMode::Regex, false))
            .expect_err("invalid regex must fail");
        assert_eq!(err.code(), ErrorCode::Syntax);
        assert_eq!(err.info.message, "invalid pattern");
        assert!(err.info.hint.is_some());
    }

    #[test]
    fn empty_pattern_is_rejected() {
        let err = PatternMatcher::build(&request("   ", SearchMatchMode::Contains, false))
            .expect_err("empty pattern must fail");
        assert_eq!(err.code(), ErrorCode::Data);
    }

    #[test]
    fn excerpt_returns_the_matching_line() {
        let ddl = "CREATE TABLE orders (\n  id INTEGER,\n  user_id INTEGER NOT NULL\n)";
        let m = matcher("user_id", SearchMatchMode::Contains, false);
        assert_eq!(excerpt_for(ddl, &m), "user_id INTEGER NOT NULL");
    }

    #[test]
    fn excerpt_is_capped_at_200_chars() {
        let long = format!("CREATE TABLE t ({} user_id INTEGER)", "x".repeat(400));
        let m = matcher("user_id", SearchMatchMode::Contains, false);
        let excerpt = excerpt_for(&long, &m);
        assert_eq!(excerpt.chars().count(), MAX_EXCERPT_CHARS);
        assert!(excerpt.ends_with('…'));
        assert!(excerpt.starts_with("CREATE TABLE"));
    }

    #[test]
    fn truncate_keeps_short_text_intact() {
        assert_eq!(truncate_chars("short", 200), "short");
        assert_eq!(truncate_chars("", 200), "");
        assert_eq!(truncate_chars("abc", 0), "");
    }

    #[test]
    fn kinds_default_to_tables_and_views() {
        assert_eq!(
            effective_kinds(&[]),
            vec![ObjectKind::Table, ObjectKind::View]
        );
        assert_eq!(
            effective_kinds(&[ObjectKind::Function]),
            vec![ObjectKind::Function]
        );
    }

    #[test]
    fn limit_is_clamped_to_documented_range() {
        assert_eq!(effective_limit(0), 1);
        assert_eq!(effective_limit(500), 500);
        assert_eq!(effective_limit(u32::MAX), MAX_HITS as usize);
    }
}
