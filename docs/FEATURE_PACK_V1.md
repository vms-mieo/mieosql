# Feature Pack v1 — Contract & Work Split

> Tài liệu điều phối cho các agent song song. **Đọc kỹ phần "Hard rules" trước khi viết code.**
> Phase A (scaffold) đã xong: DTO + chữ ký method + Tauri command + TS bindings + khung FE
> (`store` feature tab, `FeatureDialogs`, `DialogShell`, `api.ts` wrappers) đều đã compile.

Các tính năng trong gói này (ánh xạ tới danh sách Navicat):

| # | Navicat feature | Workstream | Phase |
|---|---|---|---|
| 1 | Database-wide Search | `search.rs` | B1 |
| 2 | Visual Explain | `explain.rs` | B2 |
| 3 | Data Dictionary / Documentation | `docs.rs` | B2 |
| 4 | Data Generation | `datagen.rs` | B3 |
| 5 | Data Masking | `masking.rs` (+ export streams) | B3 |
| 6 | Structure Synchronization | `sync.rs` | B4 |
| 7 | Data Synchronization | `sync.rs` | B4 |
| 8 | Data Transfer | `transfer.rs` | B5 |
| 9 | Database Modeling (ERD, snapshot) | `snapshot.rs` + FE | B5/F5 |
| 10 | Code Insight (autocomplete, hover, beautify, rename) | FE | F1 |
| 11 | Parameter queries + result pinning | FE | F1 |
| 12 | Query Builder (light) | FE | F1 |
| 13 | BI Dashboard | FE | F5 |
| 14 | Collaboration v1 (workspace bundle) | `workspace.rs` + FE | B5/F4 |

---

## 0. Hard rules (mọi agent)

1. **Chỉ sửa file thuộc quyền sở hữu của mình** (ghi trong brief của bạn). KHÔNG sửa:
   - `crates/mieosql-api/src/{lib.rs, service.rs, dto.rs, export.rs, util.rs}`
   - `apps/mieosql-desktop/src/{commands.rs, main.rs}`
   - `apps/mieosql-desktop/frontend/src/lib/{store.ts, api.ts, bindings.ts}`
   - `apps/mieosql-desktop/frontend/src/App.tsx`, `components/{Workspace,MenuBar,Ribbon,ResultPanel,EditorPane}*.tsx`
     (trừ agent F1 được sở hữu `components/EditorPane.tsx`; `lib/sql-params.ts`)
   - `features/{registry.tsx, dialogs.tsx, DialogShell.tsx}` (trừ khi brief cho phép).
2. **Không đổi chữ ký method/DTO đã frozen.** Thêm helper private trong file của mình thì OK.
   Nếu buộc phải thay đổi (ví dụ thêm enum variant) — làm cách tối thiểu, ghi rõ trong báo cáo,
   và đảm bảo `cargo check`/`tsc` vẫn xanh. Thêm field mới vào DTO phải có `#[serde(default)]`.
3. **Nhiều agent chạy song song**: `cargo`/`npm` sẽ xếp hàng chờ lock — bình thường.
   Nếu build báo lỗi ở file **không thuộc quyền của bạn**, bỏ qua lỗi đó và thử lại sau
   (agent khác đang viết dở). TUYỆT ĐỐI không sửa file của agent khác.
4. **Không chạy `cargo fmt`** (format cả workspace, đè lên agent khác). Format thủ công.
5. Code + comment bằng tiếng Anh (nhất quán codebase). UI copy tiếng Anh.
6. Test backend: thêm `#[cfg(test)] mod tests` trong file của mình (pure logic), hoặc file test
   integration MỚI dưới `crates/mieosql-api/tests/<module>_test.rs` (tên duy nhất).
   `cargo test -p mieosql-api` phải pass.
7. Không thêm dependency mới (tránh network). `regex`, `chrono`, `serde_json`, `base64` đã có.
8. Báo cáo cuối: files đã sửa, cách verify, deviations, TODO còn lại.

---

## 1. Backend cheat sheet

```rust
use crate::service::Api;                      // struct Api { /* engine */ }
use crate::dto::{ApiError, ApiResult};        // ApiResult<T> = Result<T, ApiError>
use mieosql_core::{MieoError, ErrorCode, ...};

// Lỗi có cấu trúc (đừng unwrap):
Err(MieoError::unsupported("...").into())
Err(MieoError::new(ErrorCode::Data, "msg").with_hint("...").into())
Err(MieoError::new(ErrorCode::NotFound, "table `x` not found").into())
```

Chạy query / thu thập kết quả (helper có sẵn — `crate::util`):

```rust
let out = self.collect_query(session_id, sql, params_vec, QueryMode::Simple, None).await?;
// out.columns: Vec<ColumnMeta>, out.rows: Vec<Vec<Value>>, out.stats: QueryStats
let stats = self.execute(session_id, sql).await?;   // DDL/INSERT không cần kết quả
```

Engine introspection hiện có trên `Api` (xem `service.rs`):

```
session_info(session_id) -> SessionInfo { engine, database, in_transaction, capabilities, ... }
list_databases(session_id) / list_schemas(session_id, Option<db>)
list_objects(session_id, scope, Option<ObjectKind>, Option<pattern>) -> Vec<ObjectRef>
table_columns / table_indexes / foreign_keys (session_id, scope, table)
object_ddl(session_id, scope, kind, name) -> Option<String>
begin/commit/rollback_transaction(session_id)
```

SQL helpers (pub(crate), `crate::service` / `crate::export`):

```rust
quote_ident(engine, name) -> String                 // `x` / "x"
build_select_sql(engine, scope, table, limit)       // SELECT * FROM ...
crate::export::sql_literal(engine, &Value) -> String // literal engine-aware
crate::export::cell_text(&Value) -> String           // text không placeholder
```

PRNG + fake vocabulary dùng chung (`crate::util`):

```rust
let mut rng = crate::util::Prng::new(Some(seed));
rng.next_u64(); rng.range_i64(min, max); rng.range_f64(min, max); rng.range_usize(min, max);
rng.chance(50); rng.pick(&crate::util::FIRST_NAMES);
rng.digits(8); rng.hex(16); rng.string(len, b"abc");
// FIRST_NAMES, LAST_NAMES, CITIES, COUNTRIES, COMPANIES, STREETS, WORDS
```

`Value` model: `Null | Bool(bool) | Int(String!) | UInt(String!) | Float(f64) | Decimal(String) |
Text | Bytes(base64) | Date | Time | DateTime | Json(serde_json::Value) | Uuid | Array(Vec<Value>) |
Unsupported { native_type, reason }`.
Dùng `value.to_display_string()` / `cell_text` để render text; parse số bằng `.parse::<i64>()`.

`Engine`: `MySql | MariaDb | Postgres | Sqlite`. `ObjectKind`: `Table | View | MaterializedView |
Function | Procedure | Trigger | Sequence`. `DataTypeKind` xem `core/value.rs`.

ts-rs: các DTO đã có `#[ts(export)]` + nằm trong `export_manifest` — **không tự thêm type gốc mới**
(nếu cần type phụ trợ, gắn nó vào field của DTO có sẵn; nó sẽ được export transitively).

---

## 2. Backend briefs

### B1 — `search.rs` (Database-wide Search)

Implement `Api::search_database(session_id, request) -> ApiResult<SearchResponse>`.

- Xác định tập scope cần quét:
  - MySQL/MariaDB: khi `request.databases` rỗng → `list_databases()` lọc `!is_system`; mỗi db là một scope `{database: Some(db), schema: None}`. Nếu có `databases` → chỉ các db đó.
  - Postgres: db = `session_info.database` (hoặc `request.databases`); schemas = `request.schemas` hoặc `list_schemas(session_id, Some(db))` lọc `!is_system`.
  - SQLite: scope `{}` (hoặc `request.databases` đầu tiên làm database).
- Kinds: `request.kinds` rỗng → `[Table, View]`; nếu không rỗng → dùng danh sách đó.
- Quét: `list_objects(scope, Some(kind), None)` cho từng kind; match tên theo `mode`:
  - `Contains`: ci/không ci tuỳ `case_sensitive` (dùng `to_lowercase` khi ci; KHÔNG cần regex).
  - `Prefix`: tương tự, `starts_with`.
  - `WholeWord`: biên từ là ranh giới giữa `[A-Za-z0-9_]` và ký tự khác; tách bằng char scan.
  - `Regex`: `regex::RegexBuilder` với `case_insensitive(!case_sensitive)`; lỗi regex →
    `MieoError::new(ErrorCode::Syntax, "invalid pattern").with_hint(...)`.
- `include_columns`: với mỗi object vừa match tên, KHÔNG cần match cột; nhưng với mọi object được
  quét, lấy `table_columns` và match từng tên cột; hit cột có `matched_in: Column`, `column`,
  `column_type` (native_type). Quan trọng: cột match ngay cả khi tên bảng không match.
- `include_definitions`: với các table/view, lấy `object_ddl`; nếu DDL chứa match → thêm hit
  `matched_in: Definition` + `excerpt` (dòng chứa match, cắt ≤ 200 ký tự). Đếm object đã quét.
- `limit`: tổng hits; `truncated` khi vượt. `scanned_objects` = số table/view đã liệt kê.
- Tối ưu: dừng sớm khi đủ limit (nhưng vẫn đếm scanned đến điểm dừng); tránh fetch columns cho
  hàng nghìn bảng nếu không cần — v1 chấp nhận quét tuần tự, thêm comment.
- Tests: matcher thuần (4 mode × case), `excerpt` cắt đúng; integration SQLite tuỳ chọn.

### B2 — `explain.rs` + `docs.rs` (Visual Explain, Data Dictionary)

**Explain** — `Api::explain_query(session_id, sql, analyze) -> ApiResult<ExplainPlan>`:

- Lấy `engine` từ `session_info`. `sql.trim()`; rỗng → `ErrorCode::Data`.
- MySQL/MariaDb: `EXPLAIN FORMAT=JSON {sql}` qua `collect_query(..., QueryMode::Simple, Some(1))`;
  ô đầu tiên là JSON text → `serde_json::from_str`. Dựng cây best-effort:
  đệ quy bất kỳ object/array; node khi object có `table_name` (`label = "{table_name}"`,
  `kind = access_type`, `relation = table_name`, `cost = cost_info.query_cost|read_cost`,
  `rows_estimate = rows_examined_per_scan`), detail từ `attached_condition`/`using_filesort`….
  `raw` = pretty JSON. `format = Json`, `analyzed = false`.
- Postgres: `EXPLAIN (FORMAT JSON{, ANALYZE}) {sql}` (thêm `, BUFFERS` khi analyze). Parse
  `[{ "Plan": {...} }]`; node: label = `"{Node Type}"` + `" on {Relation Name}"` nếu có;
  `cost = Total Cost`; `rows_estimate = Plan Rows`; `actual_rows = Actual Rows`;
  `actual_ms = Actual Total Time`; detail ghép `Filter`/`Index Cond`/`Hash Cond`/`Join Filter`;
  children = `Plans`. `raw` = JSON gốc pretty.
- SQLite: `EXPLAIN QUERY PLAN {sql}`; rows 4 cột điển hình `(id, parent, notused, detail)`.
  Map theo `parent` → cây; label = detail; kind = token đầu (SCAN/SEARCH/USE/CORRELATED…);
  `format = QueryPlan`; `raw` = các dòng detail nối `\n`. `analyze` bị bỏ qua.
- `total_cost` = cost root; `max_cost` = max toàn cây; `elapsed_ms` tự đo.
- Tests: parse JSON mẫu MySQL/PG (literal trong test), build cây SQLite từ rows giả (không cần DB).

**Docs** — `Api::export_documentation(session_id, options, path) -> ApiResult<DocSummary>`:

- Thu thập: `list_objects(scope, Some(Table) [+ View nếu include_views])`;
  mỗi object: `table_columns`, `table_indexes` (nếu bật), `foreign_keys` (nếu bật),
  `object_ddl` (nếu include_ddl). `comment` lấy từ `ObjectRef.comment`.
- Markdown: `# {title}`, thông tin chung (engine, database/schema, generated_at, số liệu),
  mục lục bảng, mỗi bảng `## name (≈N rows)`, bảng cột `| Column | Type | Nullable | Key | Default | Comment |`,
  indexes, FK, DDL code block. Comment cột = "glossary" (in ra trong bảng).
- HTML: self-contained, CSS nhúng inline, escape `& < > "`; KHÔNG dùng CDN/font ngoài.
- Ghi file bằng `std::fs::write` (map lỗi → `ErrorCode::Data`). `DocSummary` đếm đủ trường + bytes.
- `title` mặc định: `"{database}.{schema} — Data Dictionary"` (bỏ phần rỗng).
- Tests: renderer markdown/HTML thuần từ dữ liệu giả; HTML escaping.

### B3 — `datagen.rs` + `masking.rs` (Data Generation, Masking)

**Datagen** — `Api::generate_data(session_id, request) -> ApiResult<GenResult>`:

- `table_columns` → danh sách cột; **bỏ qua** `is_auto_increment || is_generated` → `skipped_columns`.
- Rule theo tên cột (so khớp không phân biệt hoa thường); còn lại `Auto`:
  theo `DataTypeKind` + heuristic tên (`email|mail` → Email; `phone|mobile|tel` → Phone;
  `name` → FullName; `company|org` → Company; `city|address|country`; `created|updated|date` →
  DateTimeRecent/DateRecent (90 ngày); `bool/boolean` → Bool; `uuid` → Uuid; `int*` →
  RandomInt(1, 100000); `decimal/numeric/float` → RandomDecimal(1, 99999, scale = numeric_scale
  hoặc 2); `text/varchar/char` → RandomString(min 5, max = char_max_length chặn 40) hoặc Lorem).
  Cột nullable: thỉnh thoảng sinh NULL (`rng.chance(5)`), trừ khi user chọn strategy rõ ràng.
- Sinh giá trị theo strategy (tất cả dùng 1 `Prng` chung → deterministic theo seed).
  `Uuid`: `${hex8}-${hex4}-4${hex3}-a${hex3}-${hex12}`.
  `DateRecent/DateTimeRecent`: `chrono::Utc::now() - Duration::days(rng...)`, format
  `%Y-%m-%d` / RFC3339.
- SQL: `INSERT INTO {qualified} ({cols}) VALUES (...), (...);` — identifier theo `quote_ident`,
  literal theo `sql_literal(engine, &value)`. `qualified`: Postgres ưu tiên schema, còn lại database
  (giống `build_select_sql`); rỗng thì bỏ qualifier.
- `batch_size` default 250, clamp 1..=1000. `rows` clamp 1..=200_000.
- `PreviewSql`: trả preview tối đa 1000 dòng; `truncated = rows > preview`.
  `Execute`: nếu `!session_info.in_transaction` → `execute("BEGIN")`, chạy từng batch insert,
  `COMMIT`; lỗi → `ROLLBACK` rồi trả lỗi. `sql` trả về = preview (batch đầu/capped).
- Tests: auto strategy theo kind/tên (hàm thuần), pattern renderer `{n}/{a}/{A}/{x}/{{}}`,
  determinism của `Prng`, build INSERT đúng quoting.

**Masking** — `Api::export_result_masked(session_id, request) -> ApiResult<MaskExportResult>`:

- Giống `export_result` nhưng: chạy `request.sql` streaming (`start_query`, `QueryMode::Prepared`,
  `fetch_size: 2048`), tạo `ResultExporter::create(request.format, engine, table, path)`, và
  **mask từng batch trước khi `write_rows`**. Lỗi → `exporter.abort()`.
- Map rule theo tên cột (ci). Rule không khớp cột nào → `unmatched_rules`; `masked_columns` = số
  cột khớp. Cột không có rule giữ nguyên.
- Strategy → `Value`:
  - `Null` → `Value::Null`; `Fixed` → Text.
  - `Redact { keep_start, keep_end }` → `cell_text` giữ đầu/cuối, giữa thay `*`; Text.
  - `Email/Phone/Name/Company/City/Address` → fake **ổn định theo input**:
    hash input (`DefaultHasher` + salt của strategy) → chọn từ vocabulary/pattern:
    Email `user{hash8}@example.com`; Phone `+84{9 digits from hash}`;
    Name `${FIRST_NAMES[..]} ${LAST_NAMES[..]}`; Company/City/Address từ list (Address = số nhà + STREETS).
  - `RandomString { min_len, max_len }`, `RandomInt`, `RandomDecimal` → random theo PRNG
    (per row, seed chung từ request.seed).
  - `Hash { salt }` → `format!("{:016x}", DefaultHasher)` của `salt + cell_text`; Text.
  - `DateRecent { days }` → ngày ngẫu nhiên trong `days` ngày.
- `stats` phải có `rows`, `bytes`, `path`, `elapsed_ms` (đo thời gian tổng). Truncated theo exporter.
- Tests: từng strategy thuần (input cố định → output cố định với seed), redact giữ đầu/cuối.

### B4 — `sync.rs` (Structure Sync + Data Sync)

**Structure** — `Api::structure_sync_plan(request) -> ApiResult<StructureSyncPlan>`:

- `session_info` cả 2 endpoint → engine; `same_engine = src.engine == dst.engine`.
- Bảng: `list_objects(kind=Table)` 2 bên; lọc `request.tables` (ci) nếu có.
- Với từng bảng union:
  - chỉ có source → `Added`; chỉ có target → `Removed`;
  - cả hai → so `table_columns`: match theo tên chính xác; Added/Removed/Modified (so
    `native_type.to_lowercase().trim()`, `nullable`). `Same` nếu không khác.
  - indexes (nếu include_indexes): so theo tên + columns; PK bỏ qua.
  - FK: đưa vào `fk_report` dạng `"+ fk_name (cols) → ref_table(ref_cols)"` / `"- ..."` / `"~ ..."`.
- Statements (chỉ sinh SQL chạy được; unsupported → warning, không thêm statement):
  - `CreateTable`: same-engine → `object_ddl(source)`; khác engine → generate từ columns
    (map `DataTypeKind` → type của engine đích: Postgres `bigint/numeric/text/boolean/timestamptz/bytea/double precision`,
    MySQL `BIGINT/DECIMAL(p,s)/TEXT/CHAR(1)/DATETIME/LONGBLOB/DOUBLE`, SQLite `INTEGER/TEXT/REAL/BLOB`),
    PK nếu `is_primary_key`. `DROP TABLE IF EXISTS` trước khi create khi direction tạo mới? Không —
    CreateTable chỉ khi bảng thiếu. Cross-engine → warning "best-effort".
  - `AddColumn`: `ALTER TABLE t ADD COLUMN c type [NOT NULL]` (bỏ NOT NULL nếu không default? giữ
    NOT NULL chỉ khi có default hoặc bảng rỗng — đơn giản: giữ nguyên như source, kèm warning nếu NOT NULL).
  - `DropColumn` (destructive), `AlterColumn`: MySQL `MODIFY COLUMN` (full def từ source),
    Postgres `ALTER TABLE t ALTER COLUMN c TYPE type`, SQLite → warning, bỏ qua.
  - `AddIndex`/`DropIndex`: `CREATE [UNIQUE] INDEX name ON t (cols)` /
    MySQL `DROP INDEX name ON t`; Postgres/SQLite `DROP INDEX name`.
    Index của SQLite (autoindex) → bỏ; PG constraint-backed index → bỏ (không drop được).
- Thứ tự statements khi trả về: CreateTable → AddColumn → AlterColumn → AddIndex → DropIndex →
  DropColumn → DropTable (an toàn khi apply tuần tự). Summary đếm theo `diffs` (không phải statements).
- `apply_sync_statements(session_id, statements)`: nếu session chưa trong transaction → BEGIN,
  chạy từng statement `self.execute`; dừng ở lỗi đầu tiên; rollback nếu lỗi + đã BEGIN; trả
  `applied/failed/errors/elapsed_ms`. Statement rỗng/bắt đầu `--` coi như skipped (không tính failed).

**Data** — `Api::data_sync_plan(request)` / `Api::apply_data_sync(request)`:

- Key columns: từ request; nếu rỗng → PK của bảng (từ `table_columns` source); nếu vẫn không có →
  `ErrorCode::Data` + hint "specify key_columns".
- Cột so sánh = `compare_columns` hoặc mọi cột non-key. Fetch `key + compare` cả 2 bên:
  `SELECT cols FROM t ORDER BY keys LIMIT {sample_limit+1}` (Prepared, quote_ident).
  `source_rows`/`target_rows` = số dòng đọc được.
- Hash dòng = `cell_text` join `\u{1f}`; map key→hash (key = join `\u{1f}` các cell_text của key).
- to_insert/to_update/to_delete/unchanged; nếu chạm `sample_limit` → `truncated = true` + warning.
- Statements (tối đa `max_statements`, dùng literal của **engine đích**):
  `INSERT INTO t (cols) VALUES (...);`
  `UPDATE t SET c1=lit1, ... WHERE k1=lit1 AND ...;`
  `DELETE FROM t WHERE k1=lit1 AND ...;`
  Cần full row values của source cho INSERT/UPDATE (query đủ mọi cột, không chỉ compare).
  Nếu số statement cần > max → lấy theo thứ tự insert → update → delete và đặt truncated + warning.
- `apply_data_sync`: tính lại diff (tái sử dụng hàm lõi), chạy như `apply_sync_statements`
  (BEGIN/COMMIT nếu chưa trong tx, dừng ở lỗi đầu, đếm inserted/updated/deleted/failed).
- Tests: diff thuần trên rows giả (key match, hash khác → update, thiếu → insert/delete),
  build WHERE/SET literal đúng engine; sample_limit truncation.

### B5 — `transfer.rs` + `snapshot.rs` + `workspace.rs`

**Transfer** — `Api::transfer_table(request) -> ApiResult<TransferResult>`:

- Columns nguồn = `table_columns(source)`; nếu `request.columns` rỗng → tất cả; nếu không → validate
  tồn tại (thiếu → `NotFound` + hint).
- `SELECT {cols} FROM {qualified(source)} [WHERE {where}] [LIMIT {limit}]` (build thủ công, quote_ident).
- Target: `list_objects(kind=Table)` name match (ci). Mode:
  - `CreateAppend`: thiếu → tạo bảng (same-engine: `object_ddl(source)`; khác: generate từ columns như B4);
  - `Append`: thiếu → `NotFound`;
  - `TruncateAppend`: thiếu → NotFound; `DELETE FROM t` trước khi insert;
  - `DropCreate`: `DROP TABLE IF EXISTS t` + create + insert.
- Insert batch prepared multi-row: placeholders theo engine (`?` với MySQL/SQLite, `$1..$n` với PG),
  params = flatten(Vec<Value>) — chạy qua `collect_query(target, sql, params, Prepared, None)`.
  `batch_size` default 250, clamp 1..=1000.
- Chuyển đổi giá trị: `Unsupported` → `Value::Null` + warning đếm; `Array`/`Json` với target
  MySQL/SQLite → `Value::Text(cell_text(..))` (PG giữ nguyên). Các loại khác giữ nguyên.
- `limit` → truncated. Trả rows/elapsed/target_table/created/warnings.
- Tests: build placeholder list, batch chia đúng, value conversion thuần.

**Snapshot** — `Api::schema_snapshot(session_id, scope, include_views, max_tables)`:

- Kinds = `[Table]` (+ `View`); `max_tables` default 400, clamp 1..=2000; `truncated` nếu vượt.
- Mỗi bảng: `table_columns`, `table_indexes`, `foreign_keys` (tuần tự — 1 session bị mutex),
  `rows_estimate`/`comment` từ `ObjectRef`. `generated_at` = `chrono::Utc::now().to_rfc3339()`.
- Tests: không bắt buộc (integration tuỳ chọn).

**Workspace** — `Api::export_workspace(path)` / `Api::import_workspace(path, overwrite)`:

- Export: `list_profiles()` → `WorkspaceBundle { version: 1, app: "MieoSQL", exported_at, note: None, profiles }`;
  ghi pretty JSON; trả bundle. (Profiles không chứa secret — đúng thiết kế.)
- Import: đọc + parse (lỗi → `ErrorCode::Data` + hint). Với từng profile: nếu id đã tồn tại và
  `!overwrite` → skip; ngược lại `self.engine().save_profile(&p, &ProfileSecrets::default())`
  (secret không đổi/không có). Trả `imported/skipped/names`.
- Tests: round-trip với thư mục tạm (`std::env::temp_dir()`) nếu tiện; tối thiểu test parse.

---

## 3. Frontend cheat sheet

Stack: React 19 + Tailwind 4 + CodeMirror 6 + zustand. TS **strict** (`noUnusedLocals`,
`noUnusedParameters`). Verify: `cd apps/mieosql-desktop/frontend && npx tsc --noEmit`.

- Bindings: `import type { X } from "../lib/bindings";` (barrel) hoặc `@bindings/X`.
- API wrappers có sẵn trong `lib/api.ts`: `searchDatabase, explainQuery, exportDocumentation,
  generateData, exportResultMasked, structureSyncPlan, applySyncStatements, dataSyncPlan,
  applyDataSync, transferTable, schemaSnapshot, exportWorkspace, importWorkspace, fetchRows,
  writeTextFile, writeBinaryFile`, cùng `startQuery/cancelQuery/tableColumns/listObjects/...`.
- Store (`lib/store.ts`) có sẵn:
  - `sessions: SessionInfo[]`, `activeSessionId`, `selected: SelectedObject | null`,
    `tabs`, `activeTabId`, `toast(level, text)`;
  - `openFeatureDialog(id)` / `closeFeatureDialog()`;
  - `openFeatureTab(featureId, title, props?)` — feature tab (`kind: "feature"`);
  - `explainActiveQuery(sessionId, sql)`, `requestRun`, `runWithParams(tabId, sql, values)`,
    `paramDialog`, `closeParamDialog`, `togglePin`.
- Dialog: dùng `DialogShell` (`features/DialogShell.tsx`) — `{ title, width?, onClose, children, footer? }`.
  Dialog components nhận prop `{ onClose }`; lấy session/scope/result từ store.
- Panel (feature tab): nhận `{ tab }`, đọc `tab.featureProps` (untyped `Record<string, unknown>` —
  cast an toàn: `const props = (tab.featureProps ?? {}) as { sessionId?: string; ... }`).
- Session/scope mặc định: session = tab đang active → `store.activeSessionId`; scope = `store.selected`
  (nếu cùng session) → `{}`.
- Style: Tailwind, palette slate/sky; text `text-[11px]`, heading `text-[12px] font-semibold`;
  button chuẩn: `rounded bg-sky-600 px-3 py-1.5 text-[11px] font-medium text-white hover:bg-sky-700`,
  secondary: `rounded border border-slate-300 bg-white px-3 py-1.5 text-[11px] text-slate-700 hover:bg-slate-100`.
  Bảng: xem `ObjectsView` trong `Workspace.tsx`. Icon: `lucide-react`.
- Save dialog: `import { save } from "@tauri-apps/plugin-dialog"` (đã dùng trong store.exportResult —
  xem pattern ở đó). Ghi file: `api.writeTextFile(path, contents)` / `api.writeBinaryFile(path, base64)`.
- Flow `openFeatureDialog("...")` đã nối sẵn: MenuBar Tools/Edit + Ribbon. Wire event editor:
  `window.dispatchEvent(new CustomEvent("mieosql:format-sql"))` v.v. (EditorPane lắng nghe).

---

## 4. Frontend briefs

### F1 — Code Insight & Refactoring (files: `lib/sql-format.ts`, `lib/sql-insight.ts`,
`lib/sql-params.ts` (cải tiến), `features/ParamRunDialog.tsx`, `features/QueryBuilderDialog.tsx`,
`components/EditorPane.tsx`)

EditorPane hiện có props `{ initialDoc, engine, sessionId, scope, onRun, onChange, onCursor }`
và forwardRef `{ getText, setText, focus }`. Giữ nguyên public interface (store/Workspace gọi).

- **Beautify/Minify** (`lib/sql-format.ts`): `formatSql(engine, sql): string`,
  `minifySql(engine, sql): string` (pure, deterministic, không phá literal/comment).
  EditorPane lắng nghe `mieosql:format-sql` / `mieosql:minify-sql` + keymap `Shift-Alt-f` /
  `Shift-Alt-m`; thay doc (giữ selection hợp lý), không tạo state rác.
- **Rename symbol** (`mieosql:rename-symbol`, F2): tìm word tại selection (fallback: word dưới cursor);
  nếu là identifier → `window.prompt("Rename …", word)`; thay mọi occurrence word-boundary
  (case-sensitive) trong doc bằng `changes` của một transaction. Bỏ qua token trong string/comment
  (dùng scanner đơn giản như `sql-params.ts`).
- **Autocomplete** (`lib/sql-insight.ts`):
  - `getSchema(sessionId, scope)` cache theo `sessionId + db.schema`, TTL 5 phút, dùng
    `api.schemaSnapshot(sessionId, scope, false, 200)`; trả `{tables: Map<string, ColumnDef[]>, ...}`.
  - `completionSource(engine, sessionId, getScope)` — trả CodeMirror `CompletionSource`:
    bảng (kind `class`, detail "table"), cột (kind `property`, detail native_type, boost khi có
    `alias.` trước đó hoặc trong `FROM`/`SELECT` context), keywords SQL theo engine, snippets
    (`sel`, `ins`, `upd`, `del`, `join`, `cte`, `case`) với `apply` template + cursor `#{cursor}`
    (dùng `${}` syntax của CodeMirror `snippetCompletion`).
  - Đăng ký bằng extension `autocompletion({ override: [source] })` trong EditorPane; reconfigure khi
    engine/session/scope đổi (Compartment mới).
- **Hover**: `hoverTooltip` — trỏ vào identifier: nếu là bảng → tooltip `table · {n} columns`
  (liệt kê ≤ 12 cột + type); nếu là cột (match trong schema, ưu tiên bảng đang có alias) →
  `{table}.{column} · {native_type} · PK/FK`. Fallback: keyword SQL ngắn.
- **Parameter queries** (`ParamRunDialog`): đọc `store.paramDialog` ({ tabId, sql });
  `findPlaceholders(engine, sql)` → hiển thị list input (label `$1`/`? #n`, token, giá trị);
  Enter = chạy; `Run` → `store.runWithParams(tabId, sql, values)`; `Cancel` → `closeParamDialog()`.
  Cho phép chọn kiểu (Auto/Text/Int/Decimal/Bool/Null) — mở rộng `valueFromInput` nếu cần
  (giữ chữ ký `bindPlaceholders`).
- **Query Builder** (`QueryBuilderDialog`): wizard đơn giản: chọn session + table (từ
  `schemaSnapshot` hoặc `listObjects`), check columns, thêm filter rows (column, op, value),
  order by, limit; preview SQL cập nhật live; `Insert into editor` → `store.newQueryTab(sessionId, sql)`
  (hoặc setSql tab hiện tại nếu đang là query tab) + toast.
- Tests: không có test infra FE — kiểm tra bằng tsc + code review.
- **Bắt buộc**: `npx tsc --noEmit` xanh; không sửa file ngoài danh sách trên.

### F2 — Search & Explain UI (files: `features/SearchDialog.tsx`, `features/ExplainPanel.tsx`)

- **SearchDialog**: form: pattern input (autofocus), mode select (Contains/Prefix/Whole word/Regex),
  checkbox Case sensitive / Include columns / Include definitions, giới hạn kết quả (select 100/500/2000).
  Chạy `api.searchDatabase(sessionId, request)` — session: active tab → activeSessionId (nếu chưa có
  connection → thông báo). Hiển thị: thanh thống kê (`scanned_objects`, số hits, truncated cảnh báo);
  bảng kết quả nhóm theo database/schema/object: Name, Kind badge, Matched in, Column/Type, Excerpt.
  Click row → `store.openTable(sessionId, {database, schema}, object)` và đóng dialog.
  Loading/error states rõ ràng. Enter trong input = chạy.
- **ExplainPanel**: props từ `tab.featureProps`: `{ sessionId, sql, analyze? }`.
  Tự chạy `api.explainQuery(sessionId, sql, analyze)` khi mount + nút Re-run; checkbox "Analyze
  (real execution)" chỉ hiện với Postgres (session.engine === "postgres"), toggle → chạy lại.
  Header: engine · format · elapsed · total cost · SQL (mono, truncate).
  Tree: mỗi node = row (indent theo depth, expand/collapse, chevron), label = `ExplainNode.label`,
  kind badge, relation, cost badge + thanh bar tỷ lệ `cost / max_cost` (màu sky), rows/actual rows,
  detail line (text-[10px] slate-500). Sắp xếp children theo cost giảm dần khi có cost.
  Toggle "Raw" → `<pre>` raw. Highlight node đắt nhất (viền amber). Empty/error state rõ.
- Chỉ sửa 2 file của mình.

### F3 — Data Tools UI (files: `features/DataGenDialog.tsx`, `features/MaskExportDialog.tsx`,
`features/DocExportDialog.tsx`)

- **DataGenDialog**: chọn session (select từ store.sessions), scope (mặc định từ `store.selected`,
  nếu khớp session) + database/schema inputs khi cần; table (select — `api.listObjects(sessionId,
  scope, "table")`); rows (number, 1..200k), seed (optional), mode buttons:
  `Preview` → `api.generateData(session, {..., mode: "preview_sql"})` show SQL `<pre>` (scroll,
  max-h); `Execute` → confirm (`window.confirm`) → mode `execute` → toast stats.
  Rules per column: fetch `api.tableColumns`; mỗi cột 1 select strategy (Auto/Null/Sequential/
  RandomInt/RandomDecimal/RandomString/Lorem/Email/FullName/Phone/Company/City/Country/Address/
  Bool/DateRecent/DateTimeRecent/Uuid/Pattern/Pick/Constant) + tham số nhỏ (popover/inline).
  Cột auto-increment/generated: disable + note "skipped".
- **MaskExportDialog**: nguồn = query tab active với `activeResult` có SQL: hiển thị tên tab/result,
  columns + rows preview nhỏ (3 dòng đầu — che giá trị nếu chưa chọn rule? không cần).
  Mỗi cột: select strategy (None/Null/Redact/Fixed/Email/Phone/Name/Company/City/Address/
  RandomString/Hash/RandomInt/RandomDecimal/DateRecent) + tham số (Redact: keep_start/keep_end).
  Format select (EXPORT_FORMATS) + nút Export → `save()` dialog → `api.exportResultMasked` →
  toast `masked_columns`, `unmatched_rules`, path, elapsed.
  Nếu tab không có result → empty state hướng dẫn.
- **DocExportDialog**: session + scope (từ store.selected/active), format (Markdown/HTML),
  checkboxes (columns/indexes/FKs/views/DDL), title optional; nút Export → `save()` với ext `.md`/`.html`
  → `api.exportDocumentation` → toast summary (tables/columns/bytes/path).
- Đọc store cho session/selected; không sửa file ngoài danh sách.

### F4 — Sync & Transfer UI (files: `features/StructureSyncDialog.tsx`, `features/DataSyncDialog.tsx`,
`features/TransferDialog.tsx`, `features/WorkspaceDialog.tsx`, `features/EndpointPicker.tsx`)

- **EndpointPicker** (component dùng chung trong 4 file của F4): props
  `{ label, value: SyncEndpoint | null, onChange, sessions, allowScopeInput? }`; chọn session
  (select), database + schema input/select (load `api.listDatabases`/`api.listSchemas` khi cần —
  có thể đơn giản là text input với placeholder, v1). Hiển thị engine badge.
- **StructureSyncDialog**: Source + Target endpoint (mặc định source = selected session + scope),
  tables filter (comma input, optional), include indexes checkbox; `Compare` →
  `api.structureSyncPlan({ source, target, tables, include_indexes })`. Hiển thị:
  summary chips (added/removed/changed/statements), warning banner nếu `!same_engine` hoặc có warnings;
  danh sách diff theo bảng (status icon + màu; columns +/-/~/); statements preview `<pre>` với
  destructive tô đỏ (`SyncStatement.destructive`); nút `Apply on target` (confirm; disabled nếu 0 statements)
  → `api.applySyncStatements(target.session_id, statements.map(s => s.sql))` → toast kết quả.
- **DataSyncDialog**: Source/Target/Table (text input + có thể lấy từ `store.selected`),
  key columns (comma, mặc định rỗng → BE tự lấy PK; hint rõ), compare columns optional,
  sample limit (default 100000), max statements (default 2000). `Compare` → `api.dataSyncPlan`.
  Summary cards: source rows/target rows/insert/update/delete/unchanged/truncated.
  Statements grouped theo kind, mỗi nhóm có màu (insert emerald, update amber, delete rose);
  `Apply` → confirm → `api.applyDataSync` → toast inserted/updated/deleted/failed + errors list.
- **TransferDialog**: Source endpoint + table; Target endpoint + target table name (default =
  source table) + mode select (create_append/append/truncate_append/drop_create — mô tả ngắn);
  columns subset (multi-checkbox load bằng `api.tableColumns(source)` khi table chọn); where input;
  limit; batch size; `Transfer` → confirm nếu mode destructive → `api.transferTable` → toast
  rows/elapsed/warnings.
- **WorkspaceDialog**: giải thích ngắn (an toàn: không chứa mật khẩu); `Export…` → `save()` JSON →
  `api.exportWorkspace` → toast số connections; `Import…` → `open()` (plugin-dialog cũng có `open`)
  + checkbox overwrite → `api.importWorkspace` → toast imported/skipped + danh sách tên;
  gợi ý "nhập lại mật khẩu sau khi import".
- Validate: chặn chạy khi thiếu session/table; lỗi hiển thị trong dialog (không chỉ toast).

### F5 — ERD & BI (files: `features/ErdPanel.tsx`, `features/erd-layout.ts` (tuỳ),
`features/BiPanel.tsx`, `features/charts.tsx` (tuỳ))

- **ErdPanel**: props `tab.featureProps.sessionId` (fallback `store.activeSessionId`); scope =
  props.scope ?? (store.selected nếu cùng session) ?? {}. Load `api.schemaSnapshot(sessionId, scope,
  true, 300)`. Layout (thuần TS, `erd-layout.ts`): tính degree theo FK; sắp xếp grid (số cột ≈
  ceil(sqrt(n))) theo degree giảm dần; mỗi bảng = box (header màu + danh sách cột: icon khoá cho PK,
  mũi tên cho FK, type text-[9px]). Render SVG (width/height lớn, `<g transform>` pan/zoom state):
  kéo nền để pan (pointer events), wheel zoom (clamp 0.2..3), fit button, reload button,
  search box highlight table. Edges: đường vuông góc/cubic + mũi tên đơn giản (không cần crow's foot;
  có thể vẽ endpoints chấm tròn). Legend + stats (tables/columns/FKs).
  Export: `Save SVG` → serialize `<svg>` (clone + set xmlns) → `save()` → `api.writeTextFile`;
  `Save PNG` → vẽ SVG vào `Image` + canvas (scale 2x) → `toDataURL("image/png")` → base64 →
  `api.writeBinaryFile`. Empty state (chưa chọn session / snapshot rỗng).
- **BiPanel**: dashboard nhiều tile; state lưu `localStorage["mieosql.bi.v1"]`:
  `{ tiles: [{ id, title, sessionId, sql, chart: "bar"|"line"|"pie"|"table", x, y: string[], agg:
  "sum"|"count"|"avg"|"min"|"max", width: 1|2, autoRefreshSec: 0, params: Record<string,string> }] }`.
  - Nguồn dữ liệu mỗi tile: `api.fetchRows(sessionId, sql, 5000)`; nếu sql chứa `{{param}}` →
    render input trong tile, thay trước khi fetch (escape `'` → `''`).
  - Chart options: cột X = select từ columns; cột giá trị Y (1..3) + agg; count(*) khi không chọn Y.
  - Vẽ SVG thuần trong `charts.tsx`: bar (trục X nhãn xoay nếu dài, thanh sky-500), line (polyline +
    điểm), pie (arc path + legend %), table (bảng mini). Tooltip đơn giản (`<title>`).
  - Tile UI: header (title, refresh 1 lần, menu: edit/size 1x-2x/duplicate/remove), body chart,
    footer "N rows · X ms · updated at". `Add chart` → form: session, SQL textarea (prefill SQL của
    result đang active nếu có), run/preview ngay trong form.
  - Auto refresh: `autoRefreshSec` > 0 → interval refetch (cleanup đúng). Auto tile layout = CSS grid
    `grid-cols-2`, tile width 2 = `col-span-2`.
- Không thêm dependency mới (chart SVG tự viết). `npx tsc --noEmit` phải xanh.

---

## 5. Verify & report

```bash
# Backend
cargo check -p mieosql-api && cargo test -p mieosql-api

# Frontend
cd apps/mieosql-desktop/frontend && npx tsc --noEmit
```

Báo cáo cuối mỗi agent: files thay đổi, cách đã verify, deviations so với contract, TODO.
