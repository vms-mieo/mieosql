//! Shared helpers for the feature modules (v1 feature pack).
//!
//! These wrap [`crate::service::Api`] query execution into the two shapes the
//! feature modules need: "collect the whole result" and "just execute it".

use mieosql_core::{ColumnMeta, QueryEvent, QueryMode, QueryOptions, QueryStats, Value};

use crate::dto::{ApiError, ApiResult};
use crate::service::Api;

/// A fully collected query result.
#[allow(dead_code)]
pub(crate) struct QueryOutput {
    pub columns: Vec<ColumnMeta>,
    pub rows: Vec<Vec<Value>>,
    pub stats: QueryStats,
}

#[allow(dead_code)]
impl Api {
    /// Run `sql` and collect every row into memory.
    ///
    /// Use only for bounded results (catalog queries, sync diffs with a
    /// `LIMIT`). Large user queries must keep streaming through
    /// `start_query`.
    pub(crate) async fn collect_query(
        &self,
        session_id: &str,
        sql: String,
        params: Vec<Value>,
        mode: QueryMode,
        max_rows: Option<u64>,
    ) -> ApiResult<QueryOutput> {
        let mut stream = self
            .start_query(
                session_id.to_string(),
                sql,
                params,
                Some(QueryOptions {
                    mode,
                    fetch_size: 1024,
                    max_rows,
                    timeout_ms: None,
                }),
            )
            .await?;

        let mut out = QueryOutput {
            columns: Vec::new(),
            rows: Vec::new(),
            stats: QueryStats::default(),
        };
        while let Some(event) = stream.events.recv().await {
            match event {
                QueryEvent::Columns { columns, .. } => out.columns = columns,
                QueryEvent::Batch { rows, .. } => out.rows.extend(rows),
                QueryEvent::Error { error, .. } => return Err(ApiError::from(error)),
                QueryEvent::Finished { stats, .. } => {
                    out.stats = stats;
                    break;
                }
                _ => {}
            }
        }
        Ok(out)
    }

    /// Execute a single statement with no parameters, ignoring its rows.
    pub(crate) async fn execute(&self, session_id: &str, sql: String) -> ApiResult<QueryStats> {
        let out = self
            .collect_query(session_id, sql, Vec::new(), QueryMode::Simple, None)
            .await?;
        Ok(out.stats)
    }
}

// ---------------------------------------------------------------------------
// Deterministic PRNG + fake-data vocabulary (datagen / masking)
// ---------------------------------------------------------------------------

/// Small deterministic PRNG (SplitMix64), no external dependency. The same
/// seed always produces the same sequence, so generated/masked data is
/// reproducible.
#[allow(dead_code)]
pub(crate) struct Prng {
    state: u64,
}

#[allow(dead_code)]
impl Prng {
    /// Seed from `seed`, or the wall clock when `None`.
    pub(crate) fn new(seed: Option<u64>) -> Self {
        let seed = seed.unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0x9E37_79B9_7F4A_7C15)
        });
        Self {
            state: seed ^ 0x9E37_79B9_7F4A_7C15,
        }
    }

    pub(crate) fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform integer in `[min, max]` (inclusive).
    pub(crate) fn range_i64(&mut self, min: i64, max: i64) -> i64 {
        if min >= max {
            return min;
        }
        let span = (max - min) as u64 + 1;
        min + (self.next_u64() % span) as i64
    }

    /// Uniform float in `[min, max)`.
    pub(crate) fn range_f64(&mut self, min: f64, max: f64) -> f64 {
        if min >= max {
            return min;
        }
        let unit = (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64;
        min + unit * (max - min)
    }

    /// Uniform index in `[min, max)` (usize).
    pub(crate) fn range_usize(&mut self, min: usize, max: usize) -> usize {
        if min >= max {
            return min;
        }
        min + (self.next_u64() % (max - min) as u64) as usize
    }

    /// True with `pct`% probability.
    pub(crate) fn chance(&mut self, pct: u32) -> bool {
        self.range_i64(0, 99) < i64::from(pct.min(100))
    }

    pub(crate) fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        let idx = self.range_usize(0, items.len().max(1));
        &items[idx.min(items.len().saturating_sub(1))]
    }

    /// Random string of `len` chars from `charset`.
    pub(crate) fn string(&mut self, len: usize, charset: &[u8]) -> String {
        let mut out = String::with_capacity(len);
        for _ in 0..len {
            let idx = self.range_usize(0, charset.len().max(1));
            out.push(charset[idx.min(charset.len().saturating_sub(1))] as char);
        }
        out
    }

    pub(crate) fn digits(&mut self, len: usize) -> String {
        self.string(len, b"0123456789")
    }

    pub(crate) fn hex(&mut self, len: usize) -> String {
        self.string(len, b"0123456789abcdef")
    }
}

pub(crate) const FIRST_NAMES: &[&str] = &[
    "An", "Binh", "Chi", "Dung", "Giang", "Ha", "Hung", "Khanh", "Lan", "Linh", "Mai", "Minh",
    "Nam", "Ngoc", "Phong", "Quan", "Quynh", "Son", "Trang", "Tuan", "Vy", "Yen", "Alice", "Bob",
    "Carol", "David", "Emma", "Frank", "Grace", "Henry",
];

pub(crate) const LAST_NAMES: &[&str] = &[
    "Nguyen", "Tran", "Le", "Pham", "Hoang", "Huynh", "Phan", "Vu", "Vo", "Dang", "Bui", "Do",
    "Ho", "Ngo", "Duong", "Ly", "Smith", "Johnson", "Brown", "Davis", "Miller", "Wilson",
];

pub(crate) const CITIES: &[&str] = &[
    "Ha Noi", "Ho Chi Minh City", "Da Nang", "Hai Phong", "Can Tho", "Hue", "Nha Trang", "Da Lat",
    "Vung Tau", "Bien Hoa", "Singapore", "Bangkok", "Tokyo", "Seoul", "Berlin", "Paris", "London",
    "New York",
];

pub(crate) const COUNTRIES: &[&str] = &[
    "Vietnam", "Thailand", "Singapore", "Japan", "South Korea", "Germany", "France", "USA",
    "Canada", "Australia", "Netherlands", "Spain", "Italy", "Sweden",
];

pub(crate) const COMPANIES: &[&str] = &[
    "Acme Corp", "Globex", "Initech", "Umbrella", "Hooli", "Stark Industries", "Wayne Enterprises",
    "Wonka Labs", "Cyberdyne", "Aperture Science", "Vingroup", "FPT", "Viettel", "VNG", "MoMo",
];

pub(crate) const STREETS: &[&str] = &[
    "Nguyen Hue", "Le Loi", "Tran Hung Dao", "Hai Ba Trung", "Pasteur", "Ly Thuong Kiet",
    "Kim Ma", "Xa Dan", "Main Street", "High Street", "Park Avenue", "Baker Street",
];

pub(crate) const WORDS: &[&str] = &[
    "lorem", "ipsum", "dolor", "sit", "amet", "consectetur", "adipiscing", "elit", "sed", "do",
    "eiusmod", "tempor", "incididunt", "ut", "labore", "et", "dolore", "magna", "aliqua", "enim",
    "minim", "veniam", "quis", "nostrud", "exercitation", "ullamco", "laboris", "nisi", "aliquip",
    "commodo", "consequat",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prng_is_deterministic() {
        let mut a = Prng::new(Some(42));
        let mut b = Prng::new(Some(42));
        for _ in 0..10 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn prng_ranges_are_bounded() {
        let mut rng = Prng::new(Some(7));
        for _ in 0..1000 {
            let i = rng.range_i64(-5, 5);
            assert!((-5..=5).contains(&i));
            let f = rng.range_f64(1.0, 2.0);
            assert!((1.0..2.0).contains(&f));
        }
    }

    #[test]
    fn prng_strings_have_requested_length() {
        let mut rng = Prng::new(Some(1));
        assert_eq!(rng.digits(8).len(), 8);
        assert_eq!(rng.hex(16).len(), 16);
    }
}
