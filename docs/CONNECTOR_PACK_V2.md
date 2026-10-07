# Connector Pack v2 — Contract & Work Split

> Bổ sung connector còn thiếu: **SQL Server / Azure SQL**, **MongoDB / AWS DocumentDB**,
> **Redis / ElastiCache / Memorystore**, **Apache Kafka**, cùng **cloud variants** cho
> Aurora / Redshift / Azure / DocumentDB / ElastiCache / Memorystore.
>
> Phase A (scaffold) đã xong và compile: `Engine` (+`Kafka`), `EngineKind`, `variant`,
> `Capabilities.engine_kind`, `AnyDriver` 4 driver mới + `preview_statement`,
> dependencies (tiberius / mongodb / redis / rskafka) đã thêm và build xanh.

---

## 0. Hard rules (mọi agent)

1. **Chỉ sửa file thuộc quyền của mình** (ghi trong brief). KHÔNG sửa:
   - `crates/mieosql-core/**`, `crates/mieosql-engine/**`
   - `crates/mieosql-drivers/src/{lib.rs, stream.rs, util.rs, error_map.rs, mysql.rs, postgres.rs, sqlite.rs}`
   - `crates/mieosql-api/**` (trừ khi brief cho phép rõ ràng)
   - `apps/mieosql-desktop/**`, FE shared files.
2. **Không đổi chữ ký method/DTO đã frozen** (các stub trong file của bạn). Thân hàm thì thay.
   Thêm helper private/types nội bộ file: thoải mái.
3. **Không thêm dependency** — tất cả đã có trong `crates/mieosql-drivers/Cargo.toml`:
   - `tiberius 0.13` (features: rustls, chrono, tds73)
   - `mongodb 3.9` (default: rustls-tls, dns-resolver)
   - `redis 1.7` (features: tokio-rustls-comp, tls-rustls-webpki-roots)
   - `rskafka 0.6` (features: transport-tls, compression-*)
4. **Không chạy `git`**, không `cargo fmt`. Nhiều agent song song — nếu `cargo check` báo lỗi ở
   file không thuộc quyền bạn, bỏ qua và thử lại sau. Không sửa file agent khác.
5. **Không unwrap dữ liệu từ server.** Lỗi có cấu trúc `MieoError` (ErrorCode::Connection/Auth/
   Network/Timeout/Unsupported/Data/...).
6. **Test**: unit test `#[cfg(test)]` trong file của mình cho logic thuần (parser, mapping).
   Integration test cần server thật → file `crates/mieosql-api/tests/connector_<engine>_test.rs`
   với `#[ignore = "..."]` + đọc URL từ env (`MIEOSQL_TEST_MSSQL_URL`, `MIEOSQL_TEST_MONGO_URL`,
   `MIEOSQL_TEST_REDIS_URL`, `MIEOSQL_TEST_KAFKA_BROKERS`). Không cần server để pass CI.
7. Báo cáo cuối: files, tính năng, verify, deviations, TODO.

## 1. Bối cảnh kiến trúc

- `AnyDriver` (`drivers/lib.rs`) đã có variant cho cả 7 engine và dispatch sẵn — bạn chỉ cần
  hiện thực thân các method trong file driver của mình.
- Driver struct của bạn **tự quản lý connection** (sqlx không liên quan). `run_query` nhận
  `&mut self`, `sql: String`, `params: Vec<Value>`, `options: &QueryOptions`,
  `sink: &mpsc::Sender<QueryEvent>`, `cancel: &CancellationToken`.
- Sự kiện: `QueryEvent::Started` do engine phát; driver phát `Columns { query_id, columns }`,
  `Batch { query_id, rows, batch_index }`, `Affected { rows_affected, last_insert_id }` (tuỳ),
  rồi trả `QueryStats { rows, rows_affected, elapsed_ms, truncated, last_insert_id }`.
  Xem `stream.rs` để hiểu cách SQL driver làm — nhưng **không copy** máy móc, NoSQL driver tự do
  cách stream miễn đúng event shape.
- `Value` model: `Null | Bool | Int(String) | UInt(String) | Float(f64) | Decimal(String) |
  Text | Bytes(base64) | Date | Time | DateTime | Json(serde_json::Value) | Uuid | Array | Unsupported`.
- `ColumnMeta { name, ordinal, native_type, kind: DataTypeKind, nullable: Option<bool> }`.
- `preview_statement(scope, table, limit)` — SQL engines không dùng (API layer tự sinh SQL);
  NoSQL engines trả "câu query" native để mở data grid (`preview_table` trong `api/service.rs`
  gọi nó khi `!engine.is_sql()`).
- `ConnectionSpec` có: engine, host, port, username, password, database, file, params
  (`BTreeMap<String,String>` — dùng cho option riêng), ssl_mode, variant, connect_timeout_secs.

## 2. Briefs

### C1 — SQL Server (`crates/mieosql-drivers/src/sqlserver.rs`)

- Connect: `tiberius::Config` + `tiberius::Client::connect(config, TcpStream)`. Auth SQL
  (`AuthMethod::sql_server`); `ssl_mode`: Disable → `EncryptionLevel::NotSupported/Off`;
  Prefer/Require/Verify* → TLS (`trust_cert()` cho Require; Verify* best-effort — ghi chú rõ).
  Timeout theo `spec.connect_timeout_secs`.
- Query: simple mode → `client.simple_query(sql)` stream rows; prepared → `client.query(sql, &params)`
  với tham số tiberius (`ColumnData`/`ToSql`). **Placeholder `?` phải được rewrite thành `@P1..`**
  (scanner đơn giản, bỏ qua string/comment) — viết unit test.
- Params `Vec<Value>` → tiberius: Int→I64, UInt→u64/I64, Float→F64, Decimal→Numeric/string,
  Bool→Bit, Text→String, Bytes→VarBinary, Null→None, Date/Time/DateTime→chrono types, Json→String.
- Columns metadata: lấy từ `QueryStream::columns()`; map native type → `DataTypeKind`
  (INT/BIGINT/SMALLINT/TINYINT→Integer, BIT→Bool, DECIMAL/NUMERIC/MONEY→Decimal, FLOAT/REAL→Float,
  NVARCHAR/VARCHAR/CHAR/TEXT/NTEXT→Text, VARBINARY/BINARY/IMAGE→Bytes, DATE→Date, TIME→Time,
  DATETIME/DATETIME2/SMALLDATETIME→DateTime, DATETIMEOFFSET→TimestampTz, UNIQUEIDENTIFIER→Uuid,
  XML/JSON→Json).
- Introspection (SQL chuẩn, dùng helper query nội bộ, không qua run_query public):
  databases `sys.databases`; schemas `sys.schemas`; objects `INFORMATION_SCHEMA.TABLES` (+ VIEW);
  columns `INFORMATION_SCHEMA.COLUMNS` + PK (`sys.indexes` is_primary_key); indexes `sys.indexes`
  + `sys.index_columns`; FKs `sys.foreign_keys` + `sys.foreign_key_columns`; DDL: modules
  `OBJECT_DEFINITION(OBJECT_ID(...))`, tables → generate `CREATE TABLE` cơ bản từ metadata
  (hoặc `None` + note; `ddl` capability đang true).
- Transactions: `BEGIN TRANSACTION` / `COMMIT` / `ROLLBACK` qua client, track `in_transaction`.
- `preview_statement`: không dùng — trả `Err(unsupported)` như stub (API layer sinh `TOP (n)`).
- **Được phép sửa thêm** (chỉ các hàm type-mapping/limit liên quan SQL Server, giữ nguyên
  hành vi engine khác + test hiện có):
  - `crates/mieosql-api/src/sync.rs`: hàm map `DataTypeKind` → type SQL Server trong nhánh
    generate DDL (grep `Engine::Sqlite` trong phần type mapping).
  - `crates/mieosql-api/src/transfer.rs`: hàm tương tự cho CREATE TABLE cross-engine.
- Tests: unit (placeholder rewrite, type map); integration `#[ignore]` qua `MIEOSQL_TEST_MSSQL_URL`.

### C2 — MongoDB / DocumentDB (`crates/mieosql-drivers/src/mongo.rs`)

- Connect: build URI từ spec (`mongodb://user:pass@host:port/`), options từ `spec.params`:
  `auth_source`, `replica_set`, `tls_insecure` (allow invalid certs — cần cho DocumentDB global
  bundle), `direct`, `app_name`. TLS bật khi `ssl_mode != Disable` hoặc `variant == "documentdb"`.
  `ClientOptions::parse` + `Client::with_options`; giữ default database = `spec.database` (fallback "admin").
- Query language (v1) — parser nội bộ + unit tests:
  `db.<coll>.find(<json>).limit(n)` · `db.<coll>.find(<json>)` · `db.<coll>.aggregate(<json array>)` ·
  `db.<coll>.countDocuments(<json>)` · JSON form `{ "collection": "...", "find": {...}, "limit": n }`.
  Brace-matching scanner (bỏ qua chuỗi). Lỗi parse → `ErrorCode::Syntax` + hint cú pháp.
- Streaming: batch 256 docs; **Columns = union top-level field của batch đầu** (giữ thứ tự xuất hiện,
  native_type = BSON type name, kind map: string→Text, int→Integer, long→Integer, double→Float,
  decimal128→Decimal, bool→Bool, date→DateTime, objectId→Uuid, null→Unknown, object/array→Json);
  Batch rows căn theo columns (thiếu field → Null). BSON → Value: primitives native, document/array
  → `Value::Json` (relaxed extended JSON), Decimal128 → `Value::Decimal`, ObjectId → Uuid, Binary → Bytes.
- Introspection: `list_database_names()` (+ `is_system` cho admin/local/config); collections qua
  `list_collection_names` / `listCollections` (kèm view); `table_columns` = sample 50 docs suy luận
  field; `table_indexes` = `list_indexes`; FK/DDL = rỗng/None. `use_database` = đổi db mặc định.
- `server_info`: `buildInfo` → version; `current_database`. Transactions: unsupported (capabilities false).
- `preview_statement`: `db.<coll>.find({}).limit(n)` (đã có trong stub — giữ).
- Tests: parser (nested braces, strings, aggregate), BSON→Value, column inference; integration `#[ignore]`
  qua `MIEOSQL_TEST_MONGO_URL`.

### C3 — Redis (`crates/mieosql-drivers/src/redis.rs`)

- Connect: URL `redis://[user:pass@]host:port[/db]`, TLS (`ssl_mode != Disable`) → `rediss://`.
  `redis::Client::open` + `get_multiplexed_async_connection()` (hoặc async connection — chọn cái
  ổn định với `&mut self`). `use_database` = `SELECT n`.
- Query: tokenize command (quote `'`/`"` + escape) → `redis::cmd`. **Pseudo-command `PREVIEW <key>`**:
  `TYPE` rồi fetch theo type: string→GET, hash→HGETALL, list→LRANGE 0 99, set→SMEMBERS,
  zset→ZRANGE 0 99 WITHSCORES, stream→XRANGE - + COUNT 100; rows dạng cột `field,value` (hoặc `index,value`).
- Reply → rows heuristic (ghi rõ trong code):
  Nil → 1 row null; Integer/Status/Bulk → 1 row `value`; Array scalars → rows `value`;
  Array pairs (HGETALL/ZRANGE WITHSCORES) → 2 cột `field,value`; nested arrays → flatten `c1..cn`.
  Phát Columns + Batch; `rows_affected` = số row khi command ghi (SET/DEL/...).
- Introspection: `list_databases` = 0..N (`CONFIG GET databases`, fallback 16); `list_objects(scope.database)`
  = `SELECT n` + `SCAN` (MATCH pattern nếu có, COUNT 1000, cap 1000 keys) → ObjectRef kind Table;
  `table_columns`: nếu key là hash → HKEYS (cap 100) as columns; còn lại rỗng; indexes/FK/DDL rỗng/None.
- Transactions: `MULTI` / `EXEC` / `DISCARD`, track cờ.
- `server_info`: `INFO` → parse `redis_version`, `redis_mode`, `connected_clients` (version_comment).
- `preview_statement`: `PREVIEW <key>` (đã có trong stub — giữ).
- Tests: tokenizer, reply mapping, PREVIEW chọn lệnh theo TYPE (thuần); integration `#[ignore]`
  qua `MIEOSQL_TEST_REDIS_URL`.

### C4 — Kafka (`crates/mieosql-drivers/src/kafka.rs`)

- Connect: `rskafka::client::ClientBuilder` với bootstrap `vec!["host:port"]` (spec.host có thể là
  danh sách phân tách `,`); TLS khi `ssl_mode != Disable` (`with_tls_config` — xem docs rskafka 0.6,
  feature `transport-tls`); SASL nếu có username: `SaslConfig` theo `params["sasl_mechanism"]`
  (`plain` | `scram-sha-256` | `scram-sha-512`, mặc định plain) — `Credentials`.
- `list_objects`: `client.list_topics()` → ObjectRef kind Table, comment `"N partitions"` (nếu lấy được).
- Query language (JSON, parser nội bộ + unit tests):
  - Consume: `{ "topic": "...", "partition": 0, "from": "latest"|"earliest"|<offset>, "limit": 100 }`
    → resolve offset (`get_offset(OffsetAt::Latest/Earliest)`), `fetch_records(offset, max_bytes, max_wait_ms, max_records)`
    theo vòng lặp tới `limit`; rows cột `partition, offset, timestamp, key, value` (key/value text
    hoặc base64 khi non-UTF8 — ghi chú), headers? (bỏ v1).
  - Produce: `{ "produce": { "topic": "...", "partition"?: n, "key"?: "...", "value": "..." } }`
    → `partition_client(...).produce(...)` → Affected 1.
- `server_info`: version = "Apache Kafka", version_comment = danh sách broker; `current_database` = cluster đầu.
- `list_databases`/schemas/use_database: rỗng/no-op (đã stub). Transactions: unsupported.
- `preview_statement`: JSON `{"topic","partition":0,"from":"latest","limit":n}` (đã có trong stub — giữ).
- Tests: JSON parse (from kiểu số/chữ, thiếu field → lỗi rõ), record → Value; integration `#[ignore]`
  qua `MIEOSQL_TEST_KAFKA_BROKERS`.

### C5 — FE connector UX (chờ design writer idle mới chạy)

- `apps/mieosql-desktop/frontend/src/lib/engines.ts` (mới): metadata cho 10 engine: label, kind,
  default port, implemented; **presets**: AWS Aurora (MySQL) `variant: aurora` (mysql), AWS Aurora
  (PostgreSQL) `variant: aurora` (postgres), AWS DocumentDB `variant: documentdb` (mongodb, TLS bật),
  Amazon Redshift `variant: redshift` (postgres, port 5439), Azure SQL `variant: azure` (sql_server),
  Amazon ElastiCache `variant: elasticache` (redis, TLS), Google Memorystore `variant: memorystore`
  (redis, TLS), Confluent Cloud `variant: confluent` (kafka, SASL), Aiven `variant: aiven`.
- `ConnectionModal.tsx`: engine picker nhóm SQL/NoSQL/Broker, preset chips, lưu `variant` vào
  `ProfileInput`, defaults (port/TLS/params hint: `auth_source` cho Mongo, `sasl_mechanism` cho Kafka,
  db index cho Redis), placeholder host theo preset (`.cluster-*.rds.amazonaws.com`, `*.docdb.amazonaws.com`...).
- `EditorPane.tsx`: dialect SQL Server = `MSSQL` (@codemirror/lang-sql có sẵn); engine NoSQL
  (mongodb/redis/kafka) → không gắn SQL dialect (plain text) — dùng Compartment reconfigure.
- Kiểm tra các map label engine trong Sidebar/StatusBar (nếu có) và bổ sung.
- Verify: `npx tsc --noEmit` + `npm run build`.

---

## 3. Verify chung

```bash
cargo check -p mieosql-drivers -p mieosql-api -p mieosql-desktop
cargo test  -p mieosql-api          # phải xanh toàn bộ (kể cả test cũ)
# FE (C5): cd apps/mieosql-desktop/frontend && npx tsc --noEmit && npm run build
```

Báo cáo cuối mỗi agent theo mẫu đã dùng: files / implemented / verified / deviations / TODO.
