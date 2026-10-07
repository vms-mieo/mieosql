//! Data Dictionary: export schema documentation (Markdown / HTML) from live
//! catalog metadata — tables, columns, indexes, foreign keys and DDL.
//!
//! Part of the v1 feature pack — frozen contract.

use std::time::Instant;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use mieosql_core::{
    ColumnDef, Engine, ErrorCode, ForeignKeyDef, IndexDef, MieoError, ObjectKind, Scope,
};

use crate::dto::ApiResult;
use crate::service::Api;

/// Output document format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum DocFormat {
    Markdown,
    Html,
}

/// What to include in the generated documentation.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DocOptions {
    pub scope: Scope,
    pub format: DocFormat,
    #[serde(default = "default_true")]
    pub include_columns: bool,
    #[serde(default = "default_true")]
    pub include_indexes: bool,
    #[serde(default = "default_true")]
    pub include_foreign_keys: bool,
    /// Include views alongside tables.
    #[serde(default)]
    pub include_views: bool,
    /// Include the CREATE DDL of every object (slower).
    #[serde(default)]
    pub include_ddl: bool,
    /// Document title; defaults to the scope name.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub title: Option<String>,
}

fn default_true() -> bool {
    true
}

/// What was written.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DocSummary {
    #[ts(type = "number")]
    pub tables: u64,
    #[ts(type = "number")]
    pub columns: u64,
    #[ts(type = "number")]
    pub indexes: u64,
    #[ts(type = "number")]
    pub foreign_keys: u64,
    #[ts(type = "number")]
    pub bytes: u64,
    #[ts(type = "number")]
    pub elapsed_ms: u64,
    pub path: String,
}

impl Api {
    /// Generate schema documentation for `options.scope` and write it to `path`.
    pub async fn export_documentation(
        &self,
        session_id: String,
        options: DocOptions,
        path: String,
    ) -> ApiResult<DocSummary> {
        let started = Instant::now();
        let info = self.session_info(session_id.clone()).await?;
        let scope = options.scope.clone();

        let mut refs = self
            .list_objects(
                session_id.clone(),
                scope.clone(),
                Some(ObjectKind::Table),
                None,
            )
            .await?;
        if options.include_views {
            refs.extend(
                self.list_objects(
                    session_id.clone(),
                    scope.clone(),
                    Some(ObjectKind::View),
                    None,
                )
                .await?,
            );
            refs.extend(
                self.list_objects(
                    session_id.clone(),
                    scope.clone(),
                    Some(ObjectKind::MaterializedView),
                    None,
                )
                .await?,
            );
        }

        let mut objects = Vec::with_capacity(refs.len());
        for object in &refs {
            let columns = self
                .table_columns(session_id.clone(), scope.clone(), object.name.clone())
                .await?;
            let indexes = if options.include_indexes {
                self.table_indexes(session_id.clone(), scope.clone(), object.name.clone())
                    .await?
            } else {
                Vec::new()
            };
            let foreign_keys = if options.include_foreign_keys {
                self.foreign_keys(session_id.clone(), scope.clone(), object.name.clone())
                    .await?
            } else {
                Vec::new()
            };
            let ddl = if options.include_ddl {
                self.object_ddl(
                    session_id.clone(),
                    scope.clone(),
                    object.kind,
                    object.name.clone(),
                )
                .await?
            } else {
                None
            };
            objects.push(DocObject {
                name: object.name.clone(),
                kind: object.kind,
                comment: object.comment.clone(),
                rows_estimate: object.rows_estimate,
                columns,
                indexes,
                foreign_keys,
                ddl,
            });
        }

        let generated_at = chrono::Utc::now().to_rfc3339();
        let title = options
            .title
            .as_deref()
            .map(str::trim)
            .filter(|title| !title.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| default_title(&scope));

        let view = DocView {
            engine: info.engine,
            scope: &scope,
            title: &title,
            generated_at: &generated_at,
            include_columns: options.include_columns,
            include_indexes: options.include_indexes,
            include_foreign_keys: options.include_foreign_keys,
            objects: &objects,
        };

        let contents = match options.format {
            DocFormat::Markdown => render_markdown(&view),
            DocFormat::Html => render_html(&view),
        };

        std::fs::write(&path, &contents)
            .map_err(|e| MieoError::new(ErrorCode::Data, format!("cannot write {path}: {e}")))?;

        Ok(DocSummary {
            tables: objects.len() as u64,
            columns: objects
                .iter()
                .map(|object| object.columns.len() as u64)
                .sum(),
            indexes: objects
                .iter()
                .map(|object| object.indexes.len() as u64)
                .sum(),
            foreign_keys: objects
                .iter()
                .map(|object| object.foreign_keys.len() as u64)
                .sum(),
            bytes: contents.len() as u64,
            elapsed_ms: started.elapsed().as_millis() as u64,
            path,
        })
    }
}

// ---------------------------------------------------------------------------
// Gathering model
// ---------------------------------------------------------------------------

/// One documented object with everything the renderers need.
struct DocObject {
    name: String,
    kind: ObjectKind,
    comment: Option<String>,
    rows_estimate: Option<u64>,
    columns: Vec<ColumnDef>,
    indexes: Vec<IndexDef>,
    foreign_keys: Vec<ForeignKeyDef>,
    ddl: Option<String>,
}

/// Render input shared by the Markdown / HTML writers.
struct DocView<'a> {
    engine: Engine,
    scope: &'a Scope,
    title: &'a str,
    generated_at: &'a str,
    include_columns: bool,
    include_indexes: bool,
    include_foreign_keys: bool,
    objects: &'a [DocObject],
}

/// Default title: `"{database}.{schema} — Data Dictionary"` (empty parts dropped).
fn default_title(scope: &Scope) -> String {
    let database = non_empty(scope.database.as_deref());
    let schema = non_empty(scope.schema.as_deref());
    let scope_name = match (database, schema) {
        (Some(database), Some(schema)) => format!("{database}.{schema}"),
        (Some(database), None) => database.to_string(),
        (None, Some(schema)) => schema.to_string(),
        (None, None) => String::new(),
    };
    if scope_name.is_empty() {
        "Data Dictionary".to_string()
    } else {
        format!("{scope_name} — Data Dictionary")
    }
}

fn non_empty(text: Option<&str>) -> Option<&str> {
    text.map(str::trim).filter(|text| !text.is_empty())
}

fn yes_no(flag: bool) -> &'static str {
    if flag {
        "yes"
    } else {
        "no"
    }
}

fn total_columns(objects: &[DocObject]) -> u64 {
    objects
        .iter()
        .map(|object| object.columns.len() as u64)
        .sum()
}

fn total_indexes(objects: &[DocObject]) -> u64 {
    objects
        .iter()
        .map(|object| object.indexes.len() as u64)
        .sum()
}

fn total_foreign_keys(objects: &[DocObject]) -> u64 {
    objects
        .iter()
        .map(|object| object.foreign_keys.len() as u64)
        .sum()
}

/// Key flags shown in the Columns table.
fn column_key(column: &ColumnDef) -> String {
    let mut keys: Vec<&str> = Vec::new();
    if column.is_primary_key {
        keys.push("PK");
    }
    if column.is_auto_increment {
        keys.push("AUTO");
    }
    if column.is_generated {
        keys.push("GENERATED");
    }
    if keys.is_empty() {
        "—".to_string()
    } else {
        keys.join(", ")
    }
}

/// `"{ref_schema}.{ref_table}({columns})"`, schema omitted when unknown.
fn fk_reference(foreign_key: &ForeignKeyDef) -> String {
    let table = match non_empty(foreign_key.ref_schema.as_deref()) {
        Some(schema) => format!("{schema}.{}", foreign_key.ref_table),
        None => foreign_key.ref_table.clone(),
    };
    if foreign_key.ref_columns.is_empty() {
        table
    } else {
        format!("{table}({})", foreign_key.ref_columns.join(", "))
    }
}

/// Section heading: name, kind for views, approximate row count when known.
fn object_heading(object: &DocObject) -> String {
    let mut heading = object.name.clone();
    match object.kind {
        ObjectKind::View => heading.push_str(" (view)"),
        ObjectKind::MaterializedView => heading.push_str(" (materialized view)"),
        _ => {}
    }
    if let Some(rows) = object.rows_estimate {
        heading.push_str(&format!(" (≈{} rows)", format_count(rows)));
    }
    heading
}

fn object_anchor(index: usize) -> String {
    format!("obj-{index}")
}

/// Thousands separators, used for row estimates / counts.
fn format_count(number: u64) -> String {
    let digits = number.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// Cell text safe inside a Markdown table row.
fn md_cell(text: &str) -> String {
    text.replace('|', "\\|").replace(['\r', '\n'], " ")
}

/// Escape the four characters that would break out of HTML text/attributes.
fn html_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(ch),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Markdown renderer
// ---------------------------------------------------------------------------

fn render_markdown(view: &DocView<'_>) -> String {
    let mut out = String::new();
    out.push_str(&format!("# {}\n\n", md_cell(view.title)));

    out.push_str("| Field | Value |\n");
    out.push_str("| --- | --- |\n");
    out.push_str(&format!("| Engine | {} |\n", md_cell(view.engine.label())));
    if let Some(database) = non_empty(view.scope.database.as_deref()) {
        out.push_str(&format!("| Database | {} |\n", md_cell(database)));
    }
    if let Some(schema) = non_empty(view.scope.schema.as_deref()) {
        out.push_str(&format!("| Schema | {} |\n", md_cell(schema)));
    }
    out.push_str(&format!("| Generated | {} |\n", md_cell(view.generated_at)));
    out.push_str(&format!("| Objects | {} |\n", view.objects.len()));
    out.push_str(&format!("| Columns | {} |\n", total_columns(view.objects)));
    if view.include_indexes {
        out.push_str(&format!("| Indexes | {} |\n", total_indexes(view.objects)));
    }
    if view.include_foreign_keys {
        out.push_str(&format!(
            "| Foreign keys | {} |\n",
            total_foreign_keys(view.objects)
        ));
    }
    out.push('\n');

    if !view.objects.is_empty() {
        out.push_str("## Table of contents\n\n");
        for (index, object) in view.objects.iter().enumerate() {
            out.push_str(&format!(
                "- [{}](#{})\n",
                md_cell(&object.name),
                object_anchor(index)
            ));
        }
        out.push('\n');
    }

    for (index, object) in view.objects.iter().enumerate() {
        out.push_str(&format!("<a id=\"{}\"></a>\n", object_anchor(index)));
        out.push_str(&format!("## {}\n\n", md_cell(&object_heading(object))));
        if let Some(comment) = non_empty(object.comment.as_deref()) {
            out.push_str(&format!("{}\n\n", comment.trim()));
        }
        if view.include_columns && !object.columns.is_empty() {
            markdown_columns(&mut out, &object.columns);
        }
        if view.include_indexes && !object.indexes.is_empty() {
            markdown_indexes(&mut out, &object.indexes);
        }
        if view.include_foreign_keys && !object.foreign_keys.is_empty() {
            markdown_foreign_keys(&mut out, &object.foreign_keys);
        }
        if let Some(ddl) = non_empty(object.ddl.as_deref()) {
            out.push_str("### DDL\n\n```sql\n");
            out.push_str(ddl.trim());
            out.push_str("\n```\n\n");
        }
    }

    out.push_str("---\n\n*Generated by MieoSQL*\n");
    out
}

fn markdown_columns(out: &mut String, columns: &[ColumnDef]) {
    out.push_str("### Columns\n\n");
    out.push_str("| Column | Type | Nullable | Key | Default | Comment |\n");
    out.push_str("| --- | --- | --- | --- | --- | --- |\n");
    for column in columns {
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} |\n",
            md_cell(&column.name),
            md_cell(&column.native_type),
            yes_no(column.nullable),
            md_cell(&column_key(column)),
            md_cell(column.default_value.as_deref().unwrap_or("—")),
            md_cell(column.comment.as_deref().unwrap_or("—")),
        ));
    }
    out.push('\n');
}

fn markdown_indexes(out: &mut String, indexes: &[IndexDef]) {
    out.push_str("### Indexes\n\n");
    out.push_str("| Name | Columns | Unique | Primary | Type |\n");
    out.push_str("| --- | --- | --- | --- | --- |\n");
    for index in indexes {
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} |\n",
            md_cell(&index.name),
            md_cell(&index.columns.join(", ")),
            yes_no(index.is_unique),
            yes_no(index.is_primary),
            md_cell(index.index_type.as_deref().unwrap_or("—")),
        ));
    }
    out.push('\n');
}

fn markdown_foreign_keys(out: &mut String, foreign_keys: &[ForeignKeyDef]) {
    out.push_str("### Foreign keys\n\n");
    out.push_str("| Name | Columns | References | On update | On delete |\n");
    out.push_str("| --- | --- | --- | --- | --- |\n");
    for foreign_key in foreign_keys {
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} |\n",
            md_cell(&foreign_key.name),
            md_cell(&foreign_key.columns.join(", ")),
            md_cell(&fk_reference(foreign_key)),
            md_cell(foreign_key.on_update.as_deref().unwrap_or("—")),
            md_cell(foreign_key.on_delete.as_deref().unwrap_or("—")),
        ));
    }
    out.push('\n');
}

// ---------------------------------------------------------------------------
// HTML renderer (self-contained: inline CSS, no external assets)
// ---------------------------------------------------------------------------

const HTML_CSS: &str = "\
:root { color-scheme: light; }
* { box-sizing: border-box; }
body { margin: 0 auto; padding: 2rem 2.5rem 4rem; max-width: 1100px; \
font-family: -apple-system, BlinkMacSystemFont, \"Segoe UI\", Roboto, Helvetica, Arial, sans-serif; \
color: #1e293b; line-height: 1.5; }
h1 { font-size: 1.7rem; margin: 0 0 1rem; }
h2 { font-size: 1.2rem; border-bottom: 1px solid #e2e8f0; padding-bottom: .3rem; margin-top: 2rem; }
h3 { font-size: 1rem; margin: 1rem 0 .4rem; }
a { color: #0284c7; }
dl.meta { display: grid; grid-template-columns: max-content 1fr; gap: .15rem 1rem; \
margin: 0 0 1.5rem; font-size: .85rem; }
dl.meta dt { font-weight: 600; color: #475569; }
dl.meta dd { margin: 0; }
ul.toc { columns: 2; font-size: .85rem; }
table { border-collapse: collapse; width: 100%; margin: .4rem 0 1rem; font-size: .8rem; }
th, td { border: 1px solid #e2e8f0; padding: .3rem .5rem; text-align: left; vertical-align: top; }
th { background: #f1f5f9; font-weight: 600; }
pre.ddl { background: #0f172a; color: #e2e8f0; padding: .8rem 1rem; border-radius: 6px; \
overflow-x: auto; font-size: .75rem; line-height: 1.45; }
code { font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; }
.comment { color: #475569; font-size: .85rem; }
footer { margin-top: 3rem; color: #94a3b8; font-size: .75rem; }
";

fn render_html(view: &DocView<'_>) -> String {
    let mut out = String::new();
    out.push_str("<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n");
    out.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n");
    out.push_str("<meta name=\"generator\" content=\"MieoSQL\">\n");
    out.push_str(&format!("<title>{}</title>\n", html_escape(view.title)));
    out.push_str("<style>\n");
    out.push_str(HTML_CSS);
    out.push_str("</style>\n</head>\n<body>\n");
    out.push_str(&format!("<h1>{}</h1>\n", html_escape(view.title)));

    out.push_str("<dl class=\"meta\">\n");
    html_meta(&mut out, "Engine", view.engine.label());
    if let Some(database) = non_empty(view.scope.database.as_deref()) {
        html_meta(&mut out, "Database", database);
    }
    if let Some(schema) = non_empty(view.scope.schema.as_deref()) {
        html_meta(&mut out, "Schema", schema);
    }
    html_meta(&mut out, "Generated", view.generated_at);
    html_meta(&mut out, "Objects", &view.objects.len().to_string());
    html_meta(&mut out, "Columns", &total_columns(view.objects).to_string());
    if view.include_indexes {
        html_meta(&mut out, "Indexes", &total_indexes(view.objects).to_string());
    }
    if view.include_foreign_keys {
        html_meta(
            &mut out,
            "Foreign keys",
            &total_foreign_keys(view.objects).to_string(),
        );
    }
    out.push_str("</dl>\n");

    if !view.objects.is_empty() {
        out.push_str("<h2>Table of contents</h2>\n<ul class=\"toc\">\n");
        for (index, object) in view.objects.iter().enumerate() {
            out.push_str(&format!(
                "<li><a href=\"#{}\">{}</a></li>\n",
                object_anchor(index),
                html_escape(&object.name)
            ));
        }
        out.push_str("</ul>\n");
    }

    for (index, object) in view.objects.iter().enumerate() {
        out.push_str(&format!(
            "<section class=\"object\" id=\"{}\">\n",
            object_anchor(index)
        ));
        out.push_str(&format!("<h2>{}</h2>\n", html_escape(&object_heading(object))));
        if let Some(comment) = non_empty(object.comment.as_deref()) {
            out.push_str(&format!(
                "<p class=\"comment\">{}</p>\n",
                html_escape(comment.trim())
            ));
        }
        if view.include_columns && !object.columns.is_empty() {
            html_columns(&mut out, &object.columns);
        }
        if view.include_indexes && !object.indexes.is_empty() {
            html_indexes(&mut out, &object.indexes);
        }
        if view.include_foreign_keys && !object.foreign_keys.is_empty() {
            html_foreign_keys(&mut out, &object.foreign_keys);
        }
        if let Some(ddl) = non_empty(object.ddl.as_deref()) {
            out.push_str("<h3>DDL</h3>\n");
            out.push_str(&format!(
                "<pre class=\"ddl\"><code>{}</code></pre>\n",
                html_escape(ddl.trim())
            ));
        }
        out.push_str("</section>\n");
    }

    out.push_str("<footer>Generated by MieoSQL</footer>\n</body>\n</html>\n");
    out
}

fn html_meta(out: &mut String, label: &str, value: &str) {
    out.push_str(&format!(
        "<dt>{}</dt><dd>{}</dd>\n",
        html_escape(label),
        html_escape(value)
    ));
}

fn html_cell(out: &mut String, text: &str) {
    out.push_str(&format!("<td>{}</td>", html_escape(text)));
}

fn html_columns(out: &mut String, columns: &[ColumnDef]) {
    out.push_str("<h3>Columns</h3>\n<table>\n<thead><tr>");
    for header in ["Column", "Type", "Nullable", "Key", "Default", "Comment"] {
        out.push_str(&format!("<th>{header}</th>"));
    }
    out.push_str("</tr></thead>\n<tbody>\n");
    for column in columns {
        out.push_str("<tr>");
        html_cell(out, &column.name);
        html_cell(out, &column.native_type);
        html_cell(out, yes_no(column.nullable));
        html_cell(out, &column_key(column));
        html_cell(out, column.default_value.as_deref().unwrap_or("—"));
        html_cell(out, column.comment.as_deref().unwrap_or("—"));
        out.push_str("</tr>\n");
    }
    out.push_str("</tbody>\n</table>\n");
}

fn html_indexes(out: &mut String, indexes: &[IndexDef]) {
    out.push_str("<h3>Indexes</h3>\n<table>\n<thead><tr>");
    for header in ["Name", "Columns", "Unique", "Primary", "Type"] {
        out.push_str(&format!("<th>{header}</th>"));
    }
    out.push_str("</tr></thead>\n<tbody>\n");
    for index in indexes {
        out.push_str("<tr>");
        html_cell(out, &index.name);
        html_cell(out, &index.columns.join(", "));
        html_cell(out, yes_no(index.is_unique));
        html_cell(out, yes_no(index.is_primary));
        html_cell(out, index.index_type.as_deref().unwrap_or("—"));
        out.push_str("</tr>\n");
    }
    out.push_str("</tbody>\n</table>\n");
}

fn html_foreign_keys(out: &mut String, foreign_keys: &[ForeignKeyDef]) {
    out.push_str("<h3>Foreign keys</h3>\n<table>\n<thead><tr>");
    for header in ["Name", "Columns", "References", "On update", "On delete"] {
        out.push_str(&format!("<th>{header}</th>"));
    }
    out.push_str("</tr></thead>\n<tbody>\n");
    for foreign_key in foreign_keys {
        out.push_str("<tr>");
        html_cell(out, &foreign_key.name);
        html_cell(out, &foreign_key.columns.join(", "));
        html_cell(out, &fk_reference(foreign_key));
        html_cell(out, foreign_key.on_update.as_deref().unwrap_or("—"));
        html_cell(out, foreign_key.on_delete.as_deref().unwrap_or("—"));
        out.push_str("</tr>\n");
    }
    out.push_str("</tbody>\n</table>\n");
}

#[cfg(test)]
mod tests {
    use super::*;
    use mieosql_core::DataTypeKind;

    fn sample_column(name: &str, native_type: &str, primary_key: bool) -> ColumnDef {
        ColumnDef {
            name: name.to_string(),
            ordinal: 1,
            native_type: native_type.to_string(),
            data_type: DataTypeKind::Text,
            nullable: false,
            default_value: None,
            is_primary_key: primary_key,
            is_auto_increment: false,
            is_generated: false,
            comment: None,
            char_max_length: None,
            numeric_precision: None,
            numeric_scale: None,
            collation: None,
        }
    }

    fn sample_object() -> DocObject {
        let mut id = sample_column("id", "bigint unsigned", true);
        id.is_auto_increment = true;
        let mut email = sample_column("email", "varchar(255)", false);
        email.nullable = true;
        email.comment = Some("Login email".to_string());

        DocObject {
            name: "users".to_string(),
            kind: ObjectKind::Table,
            comment: Some("Application users".to_string()),
            rows_estimate: Some(1234),
            columns: vec![id, email],
            indexes: vec![IndexDef {
                name: "idx_users_email".to_string(),
                columns: vec!["email".to_string()],
                is_unique: true,
                is_primary: false,
                index_type: Some("BTREE".to_string()),
                comment: None,
            }],
            foreign_keys: vec![ForeignKeyDef {
                name: "fk_users_role".to_string(),
                columns: vec!["role_id".to_string()],
                ref_table: "roles".to_string(),
                ref_schema: Some("public".to_string()),
                ref_columns: vec!["id".to_string()],
                on_update: Some("CASCADE".to_string()),
                on_delete: None,
            }],
            ddl: Some("CREATE TABLE `users` (`id` bigint unsigned);".to_string()),
        }
    }

    fn sample_view<'a>(
        objects: &'a [DocObject],
        scope: &'a Scope,
        title: &'a str,
    ) -> DocView<'a> {
        DocView {
            engine: Engine::MySql,
            scope,
            title,
            generated_at: "2026-01-02T03:04:05+00:00",
            include_columns: true,
            include_indexes: true,
            include_foreign_keys: true,
            objects,
        }
    }

    #[test]
    fn markdown_contains_columns_indexes_and_ddl() {
        let objects = vec![sample_object()];
        let scope = Scope {
            database: Some("shop".to_string()),
            schema: None,
        };
        let view = sample_view(&objects, &scope, "shop — Data Dictionary");
        let md = render_markdown(&view);

        assert!(md.starts_with("# shop — Data Dictionary"));
        assert!(md.contains("| Column | Type | Nullable | Key | Default | Comment |"));
        assert!(md.contains("| id | bigint unsigned | no | PK, AUTO | — | — |"));
        assert!(md.contains("| email | varchar(255) | yes | — | — | Login email |"));
        assert!(md.contains("## users (≈1,234 rows)"));
        assert!(md.contains("### Indexes"));
        assert!(md.contains("| idx_users_email | email | yes | no | BTREE |"));
        assert!(md.contains("### Foreign keys"));
        assert!(md.contains("| fk_users_role | role_id | public.roles(id) | CASCADE | — |"));
        assert!(md.contains("### DDL"));
        assert!(md.contains("```sql"));
        assert!(md.contains("| Columns | 2 |"));
        assert!(md.contains("| Indexes | 1 |"));
    }

    #[test]
    fn markdown_escapes_pipes_and_newlines() {
        assert_eq!(md_cell("a|b\nc"), "a\\|b c");

        let mut object = sample_object();
        object.columns[0].name = "we|ird".to_string();
        let objects = vec![object];
        let scope = Scope::default();
        let view = sample_view(&objects, &scope, "T");
        let md = render_markdown(&view);
        assert!(md.contains("| we\\|ird |"));
    }

    #[test]
    fn html_escapes_user_data_and_stays_self_contained() {
        let mut object = sample_object();
        object.name = "bad<name>".to_string();
        object.comment = Some("keeps & <tags> and \"quotes\"".to_string());
        object.ddl = Some("CREATE TABLE bad<name> (x TEXT DEFAULT 'a&b');".to_string());
        let objects = vec![object];
        let scope = Scope::default();
        let view = sample_view(&objects, &scope, "Title <script>");
        let html = render_html(&view);

        assert!(html.starts_with("<!DOCTYPE html>"));
        assert!(html.contains("<style>"));
        assert!(html.contains("<title>Title &lt;script&gt;</title>"));
        assert!(html.contains("bad&lt;name&gt;"));
        assert!(html.contains("keeps &amp; &lt;tags&gt; and &quot;quotes&quot;"));
        assert!(html.contains("CREATE TABLE bad&lt;name&gt; (x TEXT DEFAULT 'a&amp;b');"));
        assert!(!html.contains("<script>"));
        assert!(!html.contains("http://") && !html.contains("https://"));
    }

    #[test]
    fn html_escape_covers_the_four_entities() {
        assert_eq!(
            html_escape("<a & \"b\">"),
            "&lt;a &amp; &quot;b&quot;&gt;"
        );
    }

    #[test]
    fn default_title_omits_empty_scope_parts() {
        assert_eq!(default_title(&Scope::default()), "Data Dictionary");
        assert_eq!(default_title(&Scope::db("shop")), "shop — Data Dictionary");
        assert_eq!(
            default_title(&Scope {
                database: None,
                schema: Some("public".to_string()),
            }),
            "public — Data Dictionary"
        );
        assert_eq!(
            default_title(&Scope::db_schema("shop", "public")),
            "shop.public — Data Dictionary"
        );
        assert_eq!(
            default_title(&Scope {
                database: Some("  ".to_string()),
                schema: None,
            }),
            "Data Dictionary"
        );
    }

    #[test]
    fn render_flags_suppress_sections() {
        let objects = vec![sample_object()];
        let scope = Scope::default();
        let mut view = sample_view(&objects, &scope, "T");
        view.include_columns = false;
        view.include_indexes = false;
        view.include_foreign_keys = false;

        let md = render_markdown(&view);
        assert!(!md.contains("### Columns"));
        assert!(!md.contains("### Indexes"));
        assert!(!md.contains("### Foreign keys"));

        let html = render_html(&view);
        assert!(!html.contains("<h3>Columns</h3>"));
        assert!(!html.contains("<h3>Indexes</h3>"));
        assert!(!html.contains("<h3>Foreign keys</h3>"));
    }

    #[test]
    fn formats_counts_with_thousand_separators() {
        assert_eq!(format_count(0), "0");
        assert_eq!(format_count(999), "999");
        assert_eq!(format_count(1000), "1,000");
        assert_eq!(format_count(1234567), "1,234,567");
    }

    #[test]
    fn object_heading_marks_views_and_rows() {
        let mut view_object = sample_object();
        view_object.kind = ObjectKind::View;
        view_object.rows_estimate = None;
        assert_eq!(object_heading(&view_object), "users (view)");

        let mut materialized = sample_object();
        materialized.kind = ObjectKind::MaterializedView;
        assert_eq!(object_heading(&materialized), "users (materialized view) (≈1,234 rows)");

        let plain = sample_object();
        assert_eq!(object_heading(&plain), "users (≈1,234 rows)");
    }
}
