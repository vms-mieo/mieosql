# Roadmap — MieoSQL

Nguyên tắc: mỗi phase là một cột mốc **dùng được**, ưu tiên chiều sâu (đúng, ổn định) trước chiều rộng.

## P0 — Backend nền tảng ✅ (v0.1)

- [x] Workspace 6 crate, core value/schema model, error có cấu trúc
- [x] Driver: **SQLite, MySQL, MariaDB, PostgreSQL** (sqlx 0.9, rustls, `mysql-rsa`)
- [x] Introspection: databases, schemas, tables, views, columns, indexes, FK, DDL (MySQL/SQLite)
- [x] Query: prepared + simple mode, **streaming theo batch**, parameter binding, cancel token
- [x] Transaction: begin/commit/rollback theo session
- [x] Connection profiles (metadata) + **vault mã hoá** + query history
- [x] API façade + **TS bindings tự sinh** (ts-rs) + CLI smoke-test

## P0.5 — Frontend theo design ✅ (v0.2)

- [x] Layout 3 cột Navicat-like: titlebar/menubar/ribbon · cây connections · workspace · inspector · status bar
- [x] **Data grid kiểu Excel**: header + type badge, `#` cột, sort, resize/auto-fit, filter, virtualized, NULL/số căn phải, copy, cell viewer
- [x] SQL editor CodeMirror (dialect theo engine), Ctrl+Enter, Ln/Col
- [x] Tabs: query (editor+results) · table preview (`SELECT * … LIMIT`) · objects browser
- [x] Result tabs (Result N + Messages + Status), copy CSV/JSON lines
- [x] Inspector object (columns/indexes/DDL), History, Snippets, Limit selector, transaction UI
- [x] Connection modal (test connection, SSL mode, engine roadmap-aware)
- [x] Tauri capabilities + custom titlebar + CI frontend job

## P1 — "Dùng được hằng ngày" (một phần đã xong trong Feature Pack v1)

- [x] **SSH tunnel ✅ v0.6** (password / key / agent, `russh` direct-tcpip) + jump host + UI fields (SQL engines; NoSQL báo chưa hỗ trợ)
- [ ] SSL nâng cao: CA file, client cert, skip-verify có cảnh báo
- [x] **SQL Server** ✅ v0.5 (tiberius thuần Rust — Connector Pack v2)
- [ ] Multi-statement script: SQL splitter an toàn (quote/comment/$do$) + chạy tuần tự, dừng khi lỗi
- [ ] Hard-cancel: `KILL QUERY` (MySQL), `pg_cancel_backend` (Postgres)
- [ ] Data grid editing: diff-based UPDATE/DELETE theo PK + preview SQL (FE + BE)
- [x] **Export kết quả ra file ✅ v0.3**: CSV / Text(TSV) / JSON / JSON Lines / SQL INSERTs / **Excel (XLSX)**
- [ ] Import file (CSV/JSON) — P1
- [x] **Explain plan (FE visualizer) ✅ v0.4** (tree + cost bars + raw) + **Beautify/Minify SQL ✅ v0.4**
- [x] **Find in database ✅ v0.4** (object + column + definition, 4 match mode); routine/trigger/sequence trong explorer — còn P1; DDL Postgres — còn P1
- [ ] CI: ký/notarize macOS, phát hành artifact

## Feature Pack v1 — Code Insight · Data Tools · Sync · Modeling · BI ✅ (v0.4)

> Chi tiết hợp đồng: [FEATURE_PACK_V1.md](FEATURE_PACK_V1.md)

- [x] **Code Insight**: autocomplete schema-aware, hover info, snippets, **rename symbol**, **beautify/minify**, **query builder** (light)
- [x] **Parameter queries**: dialog nhập tham số `?` / `$n` khi chạy; **result pinning**
- [x] **Database-wide Search**: object/column/definition, Contains/Prefix/Whole word/Regex, case-sensitive
- [x] **Visual Explain**: MySQL (`FORMAT=JSON`), Postgres (`FORMAT JSON` + ANALYZE), SQLite (`QUERY PLAN`) → cây chi phí
- [x] **Data Dictionary**: export Markdown/HTML (columns/indexes/FK/DDL, glossary từ comment)
- [x] **Data Generation**: 21 strategy, auto heuristic theo tên/kiểu, seed tái lập, preview SQL / execute
- [x] **Data Masking**: export kết quả với rule che (redact/fake/hash/random…), báo cáo unmatched
- [x] **Structure Sync**: diff 2 database/schema → script ALTER (columns/indexes/tables), preview + apply có transaction
- [x] **Data Sync**: diff theo key (INSERT/UPDATE/DELETE), sample limit + statement cap, preview + apply
- [x] **Data Transfer**: copy bảng cross-engine (streaming batch, 4 mode: create/append/truncate/drop-create)
- [x] **Schema Snapshot**: 1 call đủ tables+columns+indexes+FKs (nuôi ERD + autocomplete)
- [x] **ER Diagram (v1, read-only)**: layout theo FK, pan/zoom, search, export SVG/PNG
- [x] **BI Dashboard (v1)**: tile chart bar/line/pie/table (SVG thuần), `{{param}}`, auto-refresh, lưu localStorage
- [x] **Collaboration v1**: workspace bundle export/import connection metadata (không chứa mật khẩu)

## Connector Pack v2 — SQL Server · MongoDB/DocumentDB · Redis · Kafka ✅ (v0.5)

> Chi tiết hợp đồng: [CONNECTOR_PACK_V2.md](CONNECTOR_PACK_V2.md)

- [x] **SQL Server / Azure SQL** — tiberius (thuần Rust TDS): query streaming + prepared (`?`→`@P1`), introspection (sys/INFORMATION_SCHEMA), transactions, type mapping 2 chiều; TLS rustls
- [x] **MongoDB / AWS DocumentDB** — mongodb 3.9: `db.coll.find/aggregate/countDocuments`, field inference từ sample docs, BSON→Value lossless, indexes; TLS + `auth_source`/`replica_set`
- [x] **Redis / ElastiCache / Memorystore** — redis 1.7: command console (tokenizer đầy đủ), `PREVIEW <key>` (string/hash/list/set/zset/stream), SCAN key browser, MULTI/EXEC/DISCARD
- [x] **Apache Kafka / Confluent / Aiven** — rskafka: topic browser, consume JSON (`from: earliest/latest/offset`), produce (murmur2 partitioner), SASL PLAIN/SCRAM-SHA-256/512, TLS
- [x] **Cloud presets** — `variant` trên profile: aurora (MySQL/PG), redshift, azure, documentdb, elasticache, memorystore, confluent, aiven
- [x] **EngineKind** (sql/document/key_value/message_broker) + `preview_statement` cho NoSQL; `select_limited` engine-aware (TOP cho SQL Server)

## ClickHouse ✅ (v0.6)

- [x] **Driver** — `clickhouse` 0.15 (HTTP, Lz4, rustls): query streaming qua `JSONCompactEachRowWithNamesAndTypes`, giữ nguyên Int64/UInt64/Decimal/DateTime64 (không mất precision), Array/Map/JSON → Value
- [x] **Introspection** — `system.databases` / `system.tables` (table/view/matview) / `system.columns` (Nullable, PK, default, comment) / `system.data_skipping_indices` + primary key; DDL qua `create_table_query`; FK rỗng như ClickHouse
- [x] **Bound params** — `?` server-side binding; `max_execution_time` theo timeout; cancel token
- [x] **Transfer/Sync** — type mapping → ClickHouse (Nullable wrap, `ENGINE = MergeTree ORDER BY …`); engine picker + URL parser (`clickhouse://`, `clickhouses://`) + preset ClickHouse Cloud
- [x] **Live test** — `connector_clickhouse_test` (ignored, cần `MIEOSQL_TEST_CLICKHOUSE_URL`)

## P2 — Ngang tầm công cụ chuyên nghiệp

- [x] **MongoDB + Redis driver ✅ v0.5** (Connector Pack v2: collection/key browser + command console); document tree nâng cao (edit document, aggregation builder) — P3
- [ ] Import/Export Excel/XML/Markdown/HTML
- [ ] Backup/Restore (dump SQL + restore có progress)
- [ ] Snippets/saved queries tự lưu, session restore sau khi tắt app
- [ ] Code Insight nâng cao: go-to-definition, diagnostics, completion theo alias/join chính xác
- [ ] Explain nâng cao: so sánh 2 plan, gợi ý index
- [ ] OS Keychain cho vault
- [ ] Connection group/folder/color (store + FE)
- [ ] Data grid editing + raw data view (BLOB hex/text)

## P3 — Nâng cao

- [ ] Clone database giữa 2 connection (structure + data + dry-run) — tái dùng Structure/Data Sync + Transfer
- [ ] Sync nâng cao: FK constraints, defaults/identity, conflict handling 2 chiều, resume
- [ ] Proxy HTTP/SOCKS5; Oracle (đóng gói OCI — chú ý single-file constraint)
- [ ] Server monitor (session list, kill session, process list)
- [ ] Column profiling; masking nâng cao (rule presets, áp trực tiếp khi transfer)
- [ ] Automation/scheduler cho sync/transfer/generation

## P4 — Mở rộng hệ sinh thái

- [ ] Snowflake (REST), HTTP Tunnel
- [ ] ER Modeler đầy đủ: chỉnh sửa model, logical/physical, naming convention, Data Vault 2.0
- [ ] BI nâng cao: dynamic parameters phong phú, auto tile layout, page navigation, alignment tools, data profiling
- [ ] AI Assistant (NL→SQL, explain, tối ưu query)
- [ ] Cloud sync profiles/snippets/BI (optional, mã hoá end-to-end) — thay thế workspace bundle file-based

## Nguyên tắc kỹ thuật xuyên suốt

1. **Một file chạy duy nhất** mỗi OS — mọi tính năng không được phá vỡ ràng buộc này
   (engine cần client lib như Oracle phải có phương án bundling riêng).
2. FE chỉ giao tiếp qua `mieosql-api` DTO có version + TS bindings tự sinh.
3. Mọi thao tác với dữ liệu lớn phải **streaming**.
4. Không phá backward-compat của DTO trong cùng major version.
