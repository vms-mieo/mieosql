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

## P1 — "Dùng được hằng ngày" (kế tiếp)

- [ ] **SSH tunnel** (password / key / agent, `russh`) + jump host + UI fields
- [ ] SSL nâng cao: CA file, client cert, skip-verify có cảnh báo
- [ ] **SQL Server** (tiberius) — engine thứ 5
- [ ] Multi-statement script: SQL splitter an toàn (quote/comment/$do$) + chạy tuần tự, dừng khi lỗi
- [ ] Hard-cancel: `KILL QUERY` (MySQL), `pg_cancel_backend` (Postgres)
- [ ] Data grid editing: diff-based UPDATE/DELETE theo PK + preview SQL (FE + BE)
- [x] **Export kết quả ra file ✅ v0.3**: CSV / Text(TSV) / JSON / JSON Lines / SQL INSERTs / **Excel (XLSX)** — chạy lại query server-side, streaming ra đĩa (không qua IPC, không giới hạn buffer FE), save dialog, tự xoá file lỗi dở
- [ ] Import file (CSV/JSON) — P1
- [ ] Explain plan (FE visualizer) + Beautify SQL + Saved queries
- [ ] Find in database (tìm object/tên cột); routine/trigger/sequence trong explorer; DDL Postgres
- [ ] CI: ký/notarize macOS, phát hành artifact

## P2 — Ngang tầm công cụ chuyên nghiệp

- [ ] MongoDB + Redis driver (NoSQL: document tree, key browser, command console)
- [ ] Import/Export Excel/XML/Markdown/HTML; Data Transfer giữa 2 connection (cross-engine)
- [ ] Backup/Restore (dump SQL + restore có progress)
- [ ] Snippets/saved queries, session restore sau khi tắt app
- [ ] Code Insight: autocomplete dựa trên schema + keyword; object search
- [ ] Explain plan + visualize (FE)
- [ ] OS Keychain cho vault
- [ ] Connection group/folder/color (FE + store)

## P3 — Nâng cao

- [ ] Clone database giữa 2 connection (structure + data + có dry-run)
- [ ] Structure Synchronization (diff schema → script)
- [ ] Data Synchronization 2 chiều (so sánh, conflict handling)
- [ ] Proxy HTTP/SOCKS5; Oracle (đóng gói OCI — chú ý single-file constraint)
- [ ] Server monitor (session list, kill session, process list)
- [ ] Data generation (fake data theo rule)
- [ ] Column profiling, data masking

## P4 — Mở rộng hệ sinh thái

- [ ] Snowflake (REST), HTTP Tunnel, Automation/scheduler, BI dashboard (FE), ER Modeler (FE)
- [ ] AI Assistant (NL→SQL, explain, tối ưu query)
- [ ] Cloud sync profiles/snippets (optional, mã hoá end-to-end)

## Nguyên tắc kỹ thuật xuyên suốt

1. **Một file chạy duy nhất** mỗi OS — mọi tính năng không được phá vỡ ràng buộc này
   (engine cần client lib như Oracle phải có phương án bundling riêng).
2. FE chỉ giao tiếp qua `mieosql-api` DTO có version + TS bindings tự sinh.
3. Mọi thao tác với dữ liệu lớn phải **streaming**.
4. Không phá backward-compat của DTO trong cùng major version.
