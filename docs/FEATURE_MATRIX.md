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
| Oracle | ✅ | P3 (`oracledb`/OCI — cần client lib, đóng gói phức tạp hơn) |
| Snowflake | ✅ | P4 (REST API) |
| Cloud: RDS/Aurora/Azure DB/GCP SQL/Redshift/… | ✅ | ✅ v0.5 — presets `aurora` (MySQL/PG), `redshift` (PG, port 5439), `azure` (SQL Server), `documentdb`, `elasticache`, `memorystore`; kế thừa driver gốc |

## 1. Connections & Security

| Tính năng Navicat | MieoSQL |
|---|---|
| Quản lý nhiều connection, group, folder, màu | ✅ v0.1 profile store (`profiles.json`), 🔜 group/folder/color (P1, chủ yếu FE) |
| Password **lưu mã hoá** | ✅ v0.1 vault (XChaCha20-Poly1305, master key riêng, file 0600) → 🔜 OS Keychain (P2) |
| SSH Tunnel (password / private key / agent) | P1 (`russh` — thuần Rust, không cần `ssh` binary) |
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
| Table designer (tạo/sửa cột, index, FK, comment) | P1 (sinh DDL + apply transaction) |
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
| Multi-statement script | 🔜 P1 (SQL splitter an toàn + chạy tuần tự) |
| Explain / Execution plan | ✅ v0.4 — **Visual Explain** (cây chi phí, MySQL JSON / PG JSON+ANALYZE / SQLite QUERY PLAN, raw view) |
| Query history | ✅ v0.2 (panel History, click mở lại query) |
| Snippet / Saved queries | ✅ v0.2 snippets tĩnh; 🔜 P2 snippets tự lưu |
| Code Insight / Autocomplete / Refactor | ✅ v0.4 — completion schema-aware, hover, **rename symbol**, **beautify/minify** |
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
| Sửa inline, thêm/xoá dòng, commit | 🔜 P1 (diff-based UPDATE/DELETE theo PK, trả về FE) |
| Raw data view (hex/text editor cho BLOB) | P2 (value model đã giữ Bytes base64) |
| Column profiling / data masking | ✅ v0.4 — **masked export** (redact/fake/hash/random, rule theo cột); column profiling — P3 |

## 5. Data Transfer / Sync / Dump

| Tính năng Navicat | MieoSQL |
|---|---|
| Import wizard (CSV/TXT/JSON/XML/Excel…) | 🔜 P1 CSV/JSON (thuần Rust), P2 Excel/XML |
| Export (CSV/TXT/JSON/XML/SQL/HTML/Markdown) | ✅ v0.3 — **CSV, Text(TSV), JSON, JSON Lines, SQL INSERTs, Excel (XLSX)** + save dialog, streaming server-side (không giới hạn bởi FE buffer) |
| Data Transfer (copy dữ liệu giữa 2 connection khác engine) | ✅ v0.4 — streaming batch, prepared inserts, 4 mode (create/append/truncate/drop-create), warning per value |
| Data Synchronization (2 chiều, so sánh) | ✅ v0.4 v1 — diff theo key (INSERT/UPDATE/DELETE), sample limit + statement cap, preview + apply; 2 chiều/conflict handling — P3 |
| Structure Synchronization (diff schema → script) | ✅ v0.4 — columns/indexes/tables, ALTER đúng engine, preview + apply transaction; FK report-only — P3 |
| Backup / Restore (dump & restore SQL) | P2 |
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
