# Feature Matrix — Đối chiếu Navicat Premium ↔ MieoSQL

> Nguồn: navicat.com (Navicat Premium 18), manual & feature matrix chính thức. Mục đích: chốt danh sách tính năng cần build cho MieoSQL và phân kỳ.

Ký hiệu: ✅ có trong v0.1 (backend) · 🔜 có trong roadmap (ghi rõ phase) · ➖ không cần / ngoài scope

## 0. Engine được hỗ trợ

| Engine | Navicat | MieoSQL |
|---|---|---|
| MySQL / MariaDB | ✅ | ✅ v0.1 (sqlx, `mysql-rsa` auth, rustls TLS) |
| PostgreSQL | ✅ | ✅ v0.1 (sqlx) |
| SQLite | ✅ | ✅ v0.1 (sqlx, bundled — không cần cài thêm) |
| SQL Server / Azure SQL | ✅ | ✅ v0.5 (tiberius thuần Rust, TDS; TLS rustls; Azure SQL = preset `azure`) |
| MongoDB (+ Atlas) | ✅ | ✅ v0.5 (mongodb 3.9; AWS DocumentDB = preset `documentdb` + TLS) |
| Redis / ElastiCache / Memorystore | ✅ | ✅ v0.5 (redis 1.7; presets `elasticache`/`memorystore`, TLS `rediss://`) |
| Apache Kafka (+ Confluent / Aiven) | ✅ | ✅ v0.5 (rskafka thuần Rust; SASL PLAIN/SCRAM, TLS; topic browser + consume/produce JSON) |
| ClickHouse (+ ClickHouse Cloud) | ✅ | ✅ v0.6 (clickhouse 0.15, HTTP 8123/8443; JSONCompact streaming, Decimal/Array/Nullable/Datetime64 lossless; preset `cloud`) |
| Oracle | ✅ | P3 (`oracledb`/OCI — cần client lib, đóng gói phức tạp hơn) |
| Snowflake | ✅ | P4 (REST API) |
| Cloud: RDS/Aurora/Azure DB/GCP SQL/Redshift/… | ✅ | ✅ v0.5 — presets `aurora` (MySQL/PG), `redshift` (PG, port 5439), `azure` (SQL Server), `documentdb`, `elasticache`, `memorystore`; kế thừa driver gốc |

## 1. Connections & Security

| Tính năng Navicat | MieoSQL |
|---|---|
| Quản lý nhiều connection, group, folder, màu | ✅ v0.1 profile store (`profiles.json`), 🔜 group/folder/color (P1, chủ yếu FE) |
| Password **lưu mã hoá** | ✅ v0.1 vault (XChaCha20-Poly1305, master key riêng, file 0600) → 🔜 OS Keychain (P2) |
| SSH Tunnel (password / private key / agent) | ✅ v0.6 — `russh` thuần Rust (không cần `ssh` binary), direct-tcpip; Postgres/MySQL/MariaDB/SQL Server/ClickHouse + UI fields, host key check theo `known_hosts`; NoSQL (Mongo/Redis/Kafka) báo lỗi chưa hỗ trợ |
| SSL/TLS (Require / Verify-CA / Verify-Full, client cert) | ✅ v0.1 ssl_mode; 🔜 client cert & CA file (P1) |
| HTTP Tunnel | P4 |
| Proxy (HTTP/SOCKS5) | P3 |
| Import/Export danh sách connection | ✅ v0.4 — workspace bundle (JSON, **không chứa mật khẩu**), import có overwrite/skip |
| Test connection, chỉnh sửa connection properties | ✅ v0.1 API |
| Quản lý user/role của server (GRANT…) | P2 (chỉ hiển thị/tạo user — engine-specific SQL) |

## 2. Object Explorer & Design

| Tính năng Navicat | MieoSQL |
|---|---|
| Cây database → schema → bảng/view/function/procedure/trigger/sequence | ✅ v0.1: database, schema, table, view, index, FK, column metadata + DDL (MySQL/SQLite); 🔜 routine/trigger/sequence (P1) |
| **Context menu chuột phải trên cây** (Open/New Query/Import/Export/Rename/Truncate/Drop/Copy/DDL) | ✅ v0.6 — menu ngữ cảnh theo row type, xác nhận trước thao tác phá huỷ |
| **Tạo/xoá Database, Schema** | ✅ v0.6 — New Database… (charset MySQL), Drop Database…; API `execute_batch` (PG không bọc transaction) |
| Table designer (tạo bảng: cột/PK/NN/default/auto-increment, preview DDL engine-aware) | ✅ v0.6 v1 — tạo bảng (sửa bảng — P1.5) |
| View/Routine/Trigger designer | P1–P2 |
| Generate SQL script của object | 🔜 P1 (MySQL `SHOW CREATE`, SQLite `sqlite_master.sql`; Postgres dùng catalog — P1.5) |
| Data Dictionary (xuất tài liệu schema) | ✅ v0.4 — Markdown/HTML (columns/indexes/FK/DDL, comment = glossary) |
| Tìm kiếm object toàn server (Find in Database) | ✅ v0.4 — object + column + definition, Contains/Prefix/Whole word/Regex, case-sensitive |

## 3. Query & Editor

| Tính năng Navicat | MieoSQL |
|---|---|
| Query editor, nhiều tab/kết nối | ✅ v0.2 (CodeMirror 6, dialect theo engine, Ln/Col) |
| Execute, **kết quả stream theo batch** | ✅ v0.1 (BE) + v0.2 (grid) |
| Huỷ query đang chạy | ✅ v0.2 (token; hard-cancel `KILL`/`pg_cancel_backend` — P1) |
| Parameter binding (`?` / `$1`) | ✅ v0.1 (prepared mode) + v0.4 **dialog nhập tham số** khi SQL có placeholder |
| Multi-statement script | ✅ v0.6 — **Open SQL File…**: splitter an toàn (quote/comment/dollar-quote/`#temp`), chạy tuần tự, tiến độ, dừng khi lỗi |
| Explain / Execution plan | ✅ v0.4 — **Visual Explain** (cây chi phí, MySQL JSON / PG JSON+ANALYZE / SQLite QUERY PLAN, raw view) |
| Query history | ✅ v0.2 (panel History, click mở lại query) |
| Snippet / Saved queries | ✅ v0.2 snippets tĩnh; 🔜 P2 snippets tự lưu |
| Code Insight / Autocomplete / Refactor | ✅ v0.4 — completion schema-aware, hover, **rename symbol**, **beautify/minify** |
| Fix syntax tự động (khi query lỗi cú pháp) | ✅ v0.6 — **Fix SQL Syntax** cục bộ: typo keyword/hàm (SELET→SELECT, WEHER→WHERE), `==`→`=`, dấu phẩy thừa/lặp, ngoặc/nháy chưa đóng, keyword dính (GROUPBY), AND/OR treo; **dialog Before/After** trước khi áp dụng; nút Fix trên toolbar + Edit menu + lỗi syntax trong Result |
| Visual Query Builder (kéo thả) | ✅ v0.4 (light: chọn bảng/cột/filter/order/limit → SQL); kéo-thả đồ hoạ đầy đủ — P3 |
| Result pinning | ✅ v0.4 — ghim result khỏi bị cap theo tab |
| AI Assistant (NL → SQL, giải thích query) | P4 |

## 4. Data Grid & Editing

| Tính năng Navicat | MieoSQL |
|---|---|
| Xem dữ liệu dạng grid | ✅ v0.2 — **grid kiểu Excel**: header + type badge, `#` số dòng, virtualized 100k+ dòng |
| Sort / Filter trong grid | ✅ v0.2 — sort theo cột (client-side), filter loaded rows |
| Copy cell/row, xem giá trị đầy đủ | ✅ v0.2 — Ctrl+C, double-click cell viewer (JSON pretty, base64) |
| Export kết quả | ✅ v0.2 — copy CSV / JSON lines (file export + save dialog — P1) |
| Sửa inline, thêm/xoá dòng, commit | ✅ v0.6 — **grid editing**: sửa inline (double-click), thêm/xoá dòng, highlight thay đổi, Apply/Discard theo **PK diff** (1 transaction); cần PK nằm trong kết quả |
| Raw data view (hex/text editor cho BLOB) | P2 (value model đã giữ Bytes base64) |
| Column profiling / data masking | ✅ v0.4 — **masked export** (redact/fake/hash/random, rule theo cột); column profiling — P3 |

## 5. Data Transfer / Sync / Dump

| Tính năng Navicat | MieoSQL |
|---|---|
| Import wizard (CSV/TXT/JSON/XML/Excel…) | ✅ v0.6 — **CSV + JSON** (delimiter/header/NULL marker/append-hoặc-truncate, batch parameterized); Excel/XML — P2 |
| Export (CSV/TXT/JSON/XML/SQL/HTML/Markdown) | ✅ v0.3 — **CSV, Text(TSV), JSON, JSON Lines, SQL INSERTs, Excel (XLSX)** + save dialog, streaming server-side; ✅ v0.6 — **export trực tiếp từ chuột phải trên cây** |
| Data Transfer (copy dữ liệu giữa 2 connection khác engine) | ✅ v0.4 — streaming batch, prepared inserts, 4 mode (create/append/truncate/drop-create), warning per value |
| Data Synchronization (2 chiều, so sánh) | ✅ v0.4 v1 — diff theo key (INSERT/UPDATE/DELETE), sample limit + statement cap, preview + apply; 2 chiều/conflict handling — P3 |
| Structure Synchronization (diff schema → script) | ✅ v0.4 — columns/indexes/tables, ALTER đúng engine, preview + apply transaction; FK report-only — P3 |
| Backup / Restore (dump & restore SQL) | ✅ v0.6 v1 — **Backup Database/Table → .sql** (DDL + multi-row INSERTs, chọn bảng, schema-only); restore qua **Open SQL File…** (splitter + tiến độ); pg_dump-style đầy đủ — P2 |
| Data Generation (sinh dữ liệu test) | ✅ v0.4 — 21 strategy, auto heuristic, seed tái lập, preview/execute |
| Automation / Scheduler (job định kỳ) | P4 (host OS scheduler hoặc daemon in-process) |
| Server Monitor (CPU/RAM/session list) | P3 |
| BI dashboard / chart | ✅ v0.4 v1 — tile bar/line/pie/table (SVG thuần), `{{param}}`, auto-refresh, localStorage; nâng cao — P4 |
| ER Model / Data Modeler | ✅ v0.4 v1 — ERD read-only từ FK (pan/zoom/search, export SVG/PNG); chỉnh sửa model — P4 |

## 6. Nền tảng & Đóng gói

| Yêu cầu | MieoSQL |
|---|---|
| macOS / Linux / Windows | ✅ Tauri 2 hỗ trợ cả 3; CI build matrix (P1) |
| **Một file thực thi duy nhất** | ✅ 1 binary/app mỗi OS: `.dmg`/`.app`, `.AppImage`, `.exe`; FE assets nhúng trong binary |
| Không phụ thuộc lib hệ thống | ✅ rustls + SQLite bundled; (Oracle là ngoại lệ — roadmap riêng) |
| Rust + TypeScript | ✅ Rust core; TS types **tự sinh** từ Rust (`ts-rs`), FE mọi framework |
| Offline, dữ liệu cục bộ mã hoá | ✅ vault + profiles tại thư mục app data |

## Điểm khác biệt dự kiến (so với Navicat)

- **Open core**: core engine + protocol mở, FE thay thế/lập trình được.
- **TypeScript bindings tự sinh** — FE luôn đồng bộ type với BE, không lệch schema.
- **Streaming-first**: kết quả về theo batch, không block UI với bảng lớn.
- **Kiến trúc driver pluggable**: thêm engine mới = 1 crate + 1 enum variant (xem ARCHITECTURE.md §7).
