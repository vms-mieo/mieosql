//! Script-splitter edge cases (QA-01) through the public API surface:
//! each case writes a temp `.sql` file and calls `Api::script_statements`.
//! No server needed.

use std::path::{Path, PathBuf};

use mieosql_api::maintenance::ScriptStatements;
use mieosql_api::Api;
use mieosql_engine::{Engine, EngineConfig};

struct Env {
    api: Api,
    base: PathBuf,
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn setup(tag: &str) -> Env {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut base = std::env::temp_dir();
    base.push(format!("mieosql-split-{tag}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(base.join("data")).unwrap();
    let engine = Engine::open(EngineConfig {
        data_dir: base.join("data"),
    })
    .unwrap();
    Env { api: Api::new(engine), base }
}

fn split(env: &Env, dir: &Path, name: &str, sql: &str) -> ScriptStatements {
    let path = dir.join(name);
    std::fs::write(&path, sql).unwrap();
    env.api
        .script_statements(path.to_string_lossy().into_owned())
        .unwrap()
}

fn subdir(env: &Env, tag: &str) -> PathBuf {
    let dir = env.base.join(tag);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn semicolons_inside_quotes_and_brackets_are_kept() {
    let env = setup("quotes");
    let dir = subdir(&env, "q");
    let out = split(
        &env,
        &dir,
        "q.sql",
        "SELECT 'a;b'; SELECT \"c;d\"; SELECT `e;f`; SELECT [g;h];",
    );
    assert!(out.warnings.is_empty());
    assert_eq!(out.statements, vec!["SELECT 'a;b'", "SELECT \"c;d\"", "SELECT `e;f`", "SELECT [g;h]"]);
}

#[test]
fn escaped_quotes_do_not_end_strings() {
    let env = setup("esc");
    let dir = subdir(&env, "e");
    let out = split(&env, &dir, "e.sql", "SELECT 'it''s; fine'; SELECT \"a\"\"; b\";");
    assert!(out.warnings.is_empty());
    assert_eq!(out.statements.len(), 2);
    assert_eq!(out.statements[0], "SELECT 'it''s; fine'");
}

#[test]
fn line_and_block_comments_are_stripped() {
    let env = setup("comments");
    let dir = subdir(&env, "c");
    let out = split(
        &env,
        &dir,
        "c.sql",
        "-- leading; comment\nSELECT 1; # mysql; comment\nSELECT 2; /* a;b */ SELECT 3;",
    );
    assert!(out.warnings.is_empty());
    assert_eq!(out.statements.len(), 3);
    assert_eq!(out.statements[0], "SELECT 1");
    assert_eq!(out.statements[2], "SELECT 3");
}

#[test]
fn hash_temp_tables_survive_while_hash_comments_split() {
    let env = setup("hash");
    let dir = subdir(&env, "h");
    // SQL Server `#temp` tables must not be treated as comments.
    let out = split(&env, &dir, "t.sql", "CREATE TABLE #tmp (id INT); SELECT 1;");
    assert!(out.warnings.is_empty());
    assert_eq!(out.statements.len(), 2);
    assert!(out.statements[0].contains("#tmp"));

    // `#` at line start (or followed by whitespace) is a MySQL comment.
    let out = split(&env, &dir, "c.sql", "SELECT 1;\n#trailing comment\nSELECT 2;");
    assert_eq!(out.statements, vec!["SELECT 1", "SELECT 2"]);
}

#[test]
fn dollar_quoted_bodies_survive_splitting() {
    let env = setup("dollar");
    let dir = subdir(&env, "d");
    let out = split(
        &env,
        &dir,
        "f.sql",
        "CREATE FUNCTION f() RETURNS void AS $$ BEGIN; SELECT ';'; END $$ LANGUAGE plpgsql;\n\
         CREATE FUNCTION g() RETURNS void AS $tag$ SELECT 1; $tag$ LANGUAGE sql;",
    );
    assert!(out.warnings.is_empty(), "warnings: {:?}", out.warnings);
    assert_eq!(out.statements.len(), 2);
    assert!(out.statements[0].contains("SELECT ';';"));
    assert!(out.statements[1].ends_with("LANGUAGE sql"));
}

#[test]
fn dollar_placeholders_are_not_quotes() {
    let env = setup("ph");
    let dir = subdir(&env, "p");
    let out = split(&env, &dir, "p.sql", "SELECT $1; SELECT $2;");
    assert!(out.warnings.is_empty());
    assert_eq!(out.statements, vec!["SELECT $1", "SELECT $2"]);
}

#[test]
fn multi_statement_script_with_comments_and_blank_chunks() {
    let env = setup("multi");
    let dir = subdir(&env, "m");
    let out = split(
        &env,
        &dir,
        "m.sql",
        "-- header\nCREATE TABLE t (id INT);\n\nINSERT INTO t VALUES (1);\n-- tail; comment\n",
    );
    assert!(out.warnings.is_empty());
    assert_eq!(out.statements.len(), 2);
    // Comment-only / blank input yields no statements at all.
    let out = split(&env, &dir, "blank.sql", ";\n-- just a comment;\n/* nothing */\n;\n");
    assert!(out.statements.is_empty());
    assert!(out.warnings.is_empty());
}

#[test]
fn unterminated_string_reports_warning() {
    let env = setup("warn");
    let dir = subdir(&env, "w");
    let out = split(&env, &dir, "u.sql", "SELECT 'abc");
    assert_eq!(out.statements.len(), 1);
    assert_eq!(out.warnings.len(), 1);
    assert!(out.warnings[0].contains("unterminated string"));

    let out = split(&env, &dir, "b.sql", "SELECT 1 /* never closed");
    assert_eq!(out.warnings.len(), 1);
    assert!(out.warnings[0].contains("unterminated block comment"));

    let out = split(&env, &dir, "d.sql", "CREATE FUNCTION f() AS $q$ SELECT 1");
    assert_eq!(out.warnings.len(), 1);
    assert!(out.warnings[0].contains("dollar-quoted"));
}

// Routine bodies stay whole: `;` inside BEGIN..END never splits, with or
// without a DELIMITER directive (Navicat-compatible script shape).
#[test]
fn begin_end_body_stays_whole() {
    let env = setup("beginend");
    let dir = subdir(&env, "b");
    let out = split(
        &env,
        &dir,
        "trig.sql",
        "CREATE TRIGGER trg AFTER INSERT ON t BEGIN UPDATE t SET x = 1; UPDATE t SET y = 2; END;",
    );
    assert!(out.warnings.is_empty(), "warnings: {:?}", out.warnings);
    assert_eq!(out.statements.len(), 1);
    assert!(out.statements[0].contains("UPDATE t SET x = 1; UPDATE t SET y = 2;"));
}

#[test]
fn delimiter_wrapped_procedure_file_splits_into_routines() {
    let env = setup("delimfile");
    let dir = subdir(&env, "d");
    let out = split(
        &env,
        &dir,
        "proc.sql",
        "DROP PROCEDURE IF EXISTS `p`;\n\n\
         DELIMITER $$\n\
         CREATE PROCEDURE `p`(IN a INT)\n\
         BEGIN\n\
         \tDECLARE v INT DEFAULT 1;\n\
         \tSELECT a;\n\
         END$$\n\n\
         DELIMITER ;\n\
         CALL p(1);",
    );
    assert!(out.warnings.is_empty(), "warnings: {:?}", out.warnings);
    assert_eq!(out.statements.len(), 3);
    assert_eq!(out.statements[0], "DROP PROCEDURE IF EXISTS `p`");
    assert!(out.statements[1].ends_with("END"));
    assert_eq!(out.statements[2], "CALL p(1)");
    assert!(
        !out.statements
            .iter()
            .any(|s| s.to_ascii_uppercase().contains("DELIMITER"))
    );
}
