//! Data Masking: export query results with sensitive columns masked, so
//! production data can safely reach DEV/UAT.
//!
//! Part of the v1 feature pack — frozen contract. Masking is applied to the
//! streamed `Value`s right before they are written, so files never contain
//! unmasked rows even transiently in the IPC layer.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use mieosql_core::{ColumnMeta, ErrorCode, MieoError, QueryEvent, QueryMode, QueryOptions, Value};

use crate::dto::{ApiError, ApiResult};
use crate::export::{cell_text, ExportFormat, ExportStats, ResultExporter};
use crate::service::Api;
use crate::util::{Prng, CITIES, COMPANIES, FIRST_NAMES, LAST_NAMES, STREETS};

/// Charset for `RandomString` masks (lowercase letters + digits).
const STRING_CHARSET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";

/// Masking strategy for one column.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export)]
pub enum MaskStrategy {
    Null,
    /// Keep `keep_start` leading and `keep_end` trailing chars, mask the rest.
    Redact {
        keep_start: u32,
        keep_end: u32,
    },
    /// Replace with a constant value.
    Fixed {
        value: String,
    },
    /// Fake e-mail address (same fake value per distinct input, per run).
    Email,
    Phone,
    Name,
    Company,
    City,
    Address,
    RandomString {
        min_len: u32,
        max_len: u32,
    },
    /// Deterministic pseudonym: hash of the original + optional salt.
    /// Non-cryptographic (FNV-1a/SipHash) — good for de-identification, not
    /// for security-critical irreversibility.
    Hash {
        salt: Option<String>,
    },
    RandomInt {
        min: i32,
        max: i32,
    },
    RandomDecimal {
        min: f64,
        max: f64,
        scale: u32,
    },
    DateRecent {
        days: u32,
    },
}

/// Masking rule for one result column (matched case-insensitively).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MaskColumnRule {
    pub column: String,
    pub strategy: MaskStrategy,
}

/// Re-run this SQL and export the result with masking applied.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MaskExportRequest {
    /// The SQL whose result will be exported (usually a result's recorded SQL).
    pub sql: String,
    pub format: ExportFormat,
    pub path: String,
    /// Table name used for SQL-INSERT exports.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub table: Option<String>,
    #[serde(default)]
    pub rules: Vec<MaskColumnRule>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    #[ts(type = "number | null")]
    pub seed: Option<u64>,
}

/// Export result plus masking report.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MaskExportResult {
    pub stats: ExportStats,
    /// Rules that matched a result column and were applied.
    pub masked_columns: u32,
    /// Rules whose column did not exist in the result (typos, stale config).
    #[serde(default)]
    pub unmatched_rules: Vec<String>,
}

/// Matched mask strategies, indexed by result-column position.
struct MaskPlan {
    strategies: Vec<Option<MaskStrategy>>,
}

/// Aggregated matching report for the response.
#[derive(Default)]
struct MaskReport {
    masked_columns: u32,
    unmatched: Vec<String>,
}

impl MaskPlan {
    fn build(columns: &[ColumnMeta], rules: &[MaskColumnRule], report: &mut MaskReport) -> Self {
        let mut strategies: Vec<Option<MaskStrategy>> = vec![None; columns.len()];
        for rule in rules {
            match columns
                .iter()
                .position(|column| column.name.eq_ignore_ascii_case(&rule.column))
            {
                Some(index) => {
                    // First rule wins when duplicates target the same column.
                    if strategies[index].is_none() {
                        strategies[index] = Some(rule.strategy.clone());
                    }
                }
                None => report.unmatched.push(rule.column.clone()),
            }
        }
        report.masked_columns = strategies.iter().filter(|s| s.is_some()).count() as u32;
        Self { strategies }
    }

    /// Mask one streamed batch. Columns without a rule are copied verbatim;
    /// the PRNG advances only for the strategies that need it.
    fn apply(&self, rng: &mut Prng, rows: Vec<Vec<Value>>) -> Vec<Vec<Value>> {
        rows.into_iter()
            .map(|row| {
                row.into_iter()
                    .enumerate()
                    .map(
                        |(index, value)| match self.strategies.get(index).and_then(|s| s.as_ref())
                        {
                            Some(strategy) => mask_value(rng, strategy, &value),
                            None => value,
                        },
                    )
                    .collect()
            })
            .collect()
    }
}

impl Api {
    /// Export the result of `request.sql` with the given masking rules.
    pub async fn export_result_masked(
        &self,
        session_id: String,
        request: MaskExportRequest,
    ) -> ApiResult<MaskExportResult> {
        if request.sql.trim().is_empty() {
            return Err(MieoError::new(ErrorCode::Data, "SQL is required").into());
        }

        let info = self.session_info(session_id.clone()).await?;
        let started = std::time::Instant::now();

        let mut stream = self
            .start_query(
                session_id.clone(),
                request.sql,
                Vec::new(),
                Some(QueryOptions {
                    mode: QueryMode::Prepared,
                    fetch_size: 2048,
                    max_rows: None,
                    timeout_ms: None,
                }),
            )
            .await?;

        let table = request
            .table
            .clone()
            .unwrap_or_else(|| "result".to_string());
        let mut exporter =
            ResultExporter::create(request.format, info.engine, table, request.path.into())?;

        let mut rng = Prng::new(request.seed);
        let mut plan: Option<MaskPlan> = None;
        let mut report = MaskReport::default();

        while let Some(event) = stream.events.recv().await {
            match event {
                QueryEvent::Columns { columns, .. } => {
                    plan = Some(MaskPlan::build(&columns, &request.rules, &mut report));
                    exporter.set_columns(columns)?;
                }
                QueryEvent::Batch { rows, .. } => {
                    let rows = match plan.as_ref() {
                        Some(plan) => plan.apply(&mut rng, rows),
                        None => rows,
                    };
                    exporter.write_rows(&rows)?;
                }
                QueryEvent::Error { error, .. } => {
                    exporter.abort();
                    return Err(ApiError::from(error));
                }
                QueryEvent::Finished { .. } => break,
                _ => {}
            }
        }

        let mut stats: ExportStats = exporter.finish()?;
        stats.elapsed_ms = started.elapsed().as_millis() as u64;

        Ok(MaskExportResult {
            stats,
            masked_columns: report.masked_columns,
            unmatched_rules: report.unmatched,
        })
    }
}

// ---------------------------------------------------------------------------
// Mask strategies
// ---------------------------------------------------------------------------

/// Apply one strategy to one cell.
///
/// NULL inputs stay NULL for every strategy: masking never invents values for
/// absent data (this keeps nullability semantics of the exported file).
fn mask_value(rng: &mut Prng, strategy: &MaskStrategy, value: &Value) -> Value {
    if value.is_null() {
        return Value::Null;
    }
    match strategy {
        MaskStrategy::Null => Value::Null,
        MaskStrategy::Fixed { value: fixed } => Value::Text(fixed.clone()),
        MaskStrategy::Redact {
            keep_start,
            keep_end,
        } => Value::Text(redact(&cell_text(value), *keep_start, *keep_end)),
        MaskStrategy::Email => Value::Text(format!(
            "user{:08x}@example.com",
            stable_hash("email", &cell_text(value)) & 0xffff_ffff
        )),
        MaskStrategy::Phone => Value::Text(format!(
            "+84{:09}",
            stable_hash("phone", &cell_text(value)) % 1_000_000_000
        )),
        MaskStrategy::Name => {
            let hash = stable_hash("name", &cell_text(value));
            let first = FIRST_NAMES[(hash % FIRST_NAMES.len() as u64) as usize];
            let last = LAST_NAMES[((hash >> 16) % LAST_NAMES.len() as u64) as usize];
            Value::Text(format!("{first} {last}"))
        }
        MaskStrategy::Company => {
            Value::Text(pick_vocab(COMPANIES, stable_hash("company", &cell_text(value))))
        }
        MaskStrategy::City => {
            Value::Text(pick_vocab(CITIES, stable_hash("city", &cell_text(value))))
        }
        MaskStrategy::Address => {
            let hash = stable_hash("address", &cell_text(value));
            let street = pick_vocab(STREETS, hash >> 16);
            Value::Text(format!("{} {street}", hash % 999 + 1))
        }
        MaskStrategy::RandomString { min_len, max_len } => {
            let len = random_len(rng, *min_len, *max_len);
            Value::Text(rng.string(len, STRING_CHARSET))
        }
        MaskStrategy::Hash { salt } => {
            let mut hasher = DefaultHasher::new();
            if let Some(salt) = salt {
                salt.hash(&mut hasher);
            }
            cell_text(value).hash(&mut hasher);
            Value::Text(format!("{:016x}", hasher.finish()))
        }
        MaskStrategy::RandomInt { min, max } => {
            Value::Int(rng.range_i64(i64::from(*min), i64::from(*max)))
        }
        MaskStrategy::RandomDecimal { min, max, scale } => {
            let number = if min < max {
                rng.range_f64(*min, *max)
            } else {
                *min
            };
            Value::Decimal(format!("{:.*}", (*scale).min(12) as usize, number))
        }
        MaskStrategy::DateRecent { days } => {
            let days = i64::from((*days).max(1));
            let when = chrono::Utc::now() - chrono::Duration::days(rng.range_i64(0, days));
            Value::Date(when.format("%Y-%m-%d").to_string())
        }
    }
}

/// Deterministic hash of `salt + input`, stable across runs and platforms.
///
/// Uses `DefaultHasher` (SipHash with fixed keys) — deliberately **not**
/// cryptographically secure; see [`MaskStrategy::Hash`].
fn stable_hash(salt: &str, input: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    salt.hash(&mut hasher);
    input.hash(&mut hasher);
    hasher.finish()
}

/// Keep `keep_start` leading and `keep_end` trailing characters, replacing the
/// middle with `*`. Overlapping keeps leave the text unchanged.
fn redact(text: &str, keep_start: u32, keep_end: u32) -> String {
    let chars: Vec<char> = text.chars().collect();
    let start = (keep_start as usize).min(chars.len());
    let end = (keep_end as usize).min(chars.len() - start);
    let masked = chars.len() - start - end;
    let mut out = String::with_capacity(chars.len());
    out.extend(chars[..start].iter());
    out.extend(std::iter::repeat('*').take(masked));
    out.extend(chars[chars.len() - end..].iter());
    out
}

/// Pick a stable vocabulary item from a hash.
fn pick_vocab(items: &[&str], hash: u64) -> String {
    items[(hash % items.len() as u64) as usize].to_string()
}

/// Random length in `[min, max]`; tolerates inverted bounds and caps the
/// length so a pathological request cannot allocate unbounded strings.
fn random_len(rng: &mut Prng, min: u32, max: u32) -> usize {
    const MAX_LEN: usize = 4096;
    let min = (min as usize).min(MAX_LEN);
    let max = (max as usize).min(MAX_LEN);
    if min >= max {
        return min;
    }
    rng.range_usize(min, max.saturating_add(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mieosql_core::DataTypeKind;

    fn masked(strategy: &MaskStrategy, input: &str) -> String {
        let mut rng = Prng::new(Some(42));
        mask_value(&mut rng, strategy, &Value::Text(input.to_string())).to_display_string()
    }

    fn column(name: &str) -> ColumnMeta {
        ColumnMeta {
            name: name.to_string(),
            ordinal: 0,
            native_type: "text".to_string(),
            kind: DataTypeKind::Text,
            nullable: None,
        }
    }

    #[test]
    fn null_and_fixed_strategies() {
        let mut rng = Prng::new(Some(1));
        assert_eq!(
            mask_value(&mut rng, &MaskStrategy::Null, &Value::Text("secret".into())),
            Value::Null
        );
        assert_eq!(
            mask_value(
                &mut rng,
                &MaskStrategy::Fixed {
                    value: "X".into()
                },
                &Value::Text("secret".into())
            ),
            Value::Text("X".into())
        );
        // NULL inputs stay NULL whatever the strategy.
        assert_eq!(
            mask_value(&mut rng, &MaskStrategy::Email, &Value::Null),
            Value::Null
        );
    }

    #[test]
    fn redact_keeps_edges() {
        assert_eq!(redact("1234567890", 2, 3), "12*****890");
        assert_eq!(redact("abc", 0, 0), "***");
        assert_eq!(redact("abc", 5, 5), "abc");
        assert_eq!(redact("", 1, 1), "");
        // Character-based, not byte-based.
        assert_eq!(redact("héllo", 1, 1), "h***o");
    }

    #[test]
    fn fake_strategies_are_stable_per_input() {
        let strategies = [
            MaskStrategy::Email,
            MaskStrategy::Phone,
            MaskStrategy::Name,
            MaskStrategy::Company,
            MaskStrategy::City,
            MaskStrategy::Address,
        ];
        for strategy in &strategies {
            assert_eq!(masked(strategy, "alice@corp.com"), masked(strategy, "alice@corp.com"));
        }
        assert_ne!(
            masked(&MaskStrategy::Email, "alice@corp.com"),
            masked(&MaskStrategy::Email, "bob@corp.com")
        );
        assert_ne!(
            masked(&MaskStrategy::Phone, "alice@corp.com"),
            masked(&MaskStrategy::Phone, "bob@corp.com")
        );
    }

    #[test]
    fn fake_strategy_formats() {
        let email = masked(&MaskStrategy::Email, "x");
        assert!(email.starts_with("user") && email.ends_with("@example.com"));
        assert_eq!(email.len(), "user00000000@example.com".len());

        let phone = masked(&MaskStrategy::Phone, "x");
        assert!(phone.starts_with("+84"));
        assert_eq!(phone.len(), 12);

        let address = masked(&MaskStrategy::Address, "x");
        let (number, street) = address.split_once(' ').expect("number + street");
        assert!(number.parse::<u64>().is_ok());
        assert!(STREETS.contains(&street));
    }

    #[test]
    fn hash_strategy_is_deterministic_and_salted() {
        let none = masked(&MaskStrategy::Hash { salt: None }, "value");
        assert_eq!(none.len(), 16);
        assert_eq!(none, masked(&MaskStrategy::Hash { salt: None }, "value"));

        let s1 = masked(
            &MaskStrategy::Hash {
                salt: Some("s1".into()),
            },
            "value",
        );
        let s2 = masked(
            &MaskStrategy::Hash {
                salt: Some("s2".into()),
            },
            "value",
        );
        assert_ne!(s1, s2);
        assert_eq!(
            s1,
            masked(
                &MaskStrategy::Hash {
                    salt: Some("s1".into())
                },
                "value"
            )
        );
    }

    #[test]
    fn random_strategies_follow_the_seed() {
        let run = |seed: u64| {
            let mut rng = Prng::new(Some(seed));
            vec![
                mask_value(
                    &mut rng,
                    &MaskStrategy::RandomString {
                        min_len: 8,
                        max_len: 8,
                    },
                    &Value::Text("x".into()),
                )
                .to_display_string(),
                mask_value(
                    &mut rng,
                    &MaskStrategy::RandomInt { min: 1, max: 100 },
                    &Value::Text("x".into()),
                )
                .to_display_string(),
            ]
        };
        assert_eq!(run(7), run(7));
        assert_eq!(run(7)[0].len(), 8);
        let int_value: i64 = run(7)[1].parse().expect("int mask");
        assert!((1..=100).contains(&int_value));
    }

    #[test]
    fn date_recent_stays_inside_window() {
        let out = masked(&MaskStrategy::DateRecent { days: 30 }, "x");
        let parsed = chrono::NaiveDate::parse_from_str(&out, "%Y-%m-%d").expect("date mask");
        let today = chrono::Utc::now().date_naive();
        let oldest = today - chrono::Duration::days(30);
        assert!(parsed >= oldest && parsed <= today, "{out} outside window");
    }

    #[test]
    fn plan_matches_columns_case_insensitively() {
        let columns = vec![column("Email"), column("phone")];
        let rules = vec![
            MaskColumnRule {
                column: "EMAIL".to_string(),
                strategy: MaskStrategy::Null,
            },
            MaskColumnRule {
                column: "missing".to_string(),
                strategy: MaskStrategy::Null,
            },
        ];
        let mut report = MaskReport::default();
        let plan = MaskPlan::build(&columns, &rules, &mut report);
        assert_eq!(report.masked_columns, 1);
        assert_eq!(report.unmatched, vec!["missing".to_string()]);

        let rows = plan.apply(
            &mut Prng::new(Some(1)),
            vec![vec![Value::Text("a".into()), Value::Text("b".into())]],
        );
        assert_eq!(rows[0][0], Value::Null);
        assert_eq!(rows[0][1], Value::Text("b".into()));
    }
}
