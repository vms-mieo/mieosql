# MieoSQL

A Navicat-class, multi-engine database client. **Rust core + TypeScript UI, one single executable per OS** (macOS / Linux / Windows).

> **Trạng thái: v0.2 — Backend v0.1 + Frontend theo design (React/TypeScript).** UI 3 cột theo mockup Navicat Premium Lite: cây connection → editor SQL → **grid kết quả kiểu Excel** (header cột + type badge, sort, resize, filter, virtualized, copy/export), inspector object, history, snippets, transactions, status bar. Tauri 2 IPC + TS bindings tự sinh.

## Tính năng

**Backend (Rust)**
- **Engines**: SQLite (bundled), MySQL, MariaDB, PostgreSQL — thuần Rust (sqlx 0.9 + rustls), không phụ thuộc OpenSSL/client library.
- **Connections**: profile store tách secrets; vault XChaCha20-Poly1305; test connection; SSL modes.
- **Query**: prepared + simple mode, parameter binding, **streaming theo batch**, `max_rows` (Limit), timeout, **cancellation**, transaction begin/commit/rollback theo session.
- **Introspection**: databases, schemas (PG), tables/views/matviews/sequences, columns, indexes, FK, DDL.
- **Value model** lossless: BIGINT/decimal đi dạng string (không mất precision > 2^53), bytes base64, datetime RFC3339, Postgres array.
- **History** (JSONL + RAM), **TS bindings tự sinh** (ts-rs).

**Frontend (TypeScript)**
- **Data grid kiểu Excel**: cột rõ ràng với type badge (INT/STR/DEC/DATE/JSON…), cột `#` số dòng, sort theo cột, **resize cột** (double-click = auto-fit), zebra + selection, NULL kiểu `[NULL]`, số căn phải, **virtualized** (mượt với 100k+ dòng), filter client-side, copy cell/row (Ctrl+C), double-click = xem full value (JSON pretty, base64 note).
- **SQL editor**: CodeMirror 6, syntax highlight theo engine (PG/MySQL/SQLite dialect), Ctrl+Enter chạy, Ln/Col status.
- **Cây connections**: profile → databases → schemas (PG) → Tables/Views → double-click mở data grid (SELECT + LIMIT với identifier quoting chuẩn từng engine).
- **Result tabs**: nhiều result/lần chạy · Messages (log + lỗi) · Status (stats chi tiết).
- **Object inspector**: columns (PK/NN badges), indexes, nút Open Data / DDL (DDL mở thành query tab).
- **History & Snippets** panel; transaction controls; Limit selector (100…No limit); copy CSV/JSON.
- **Export kết quả ra file**: CSV · Text (TSV) · JSON · JSON Lines · SQL INSERTs · **Excel (XLSX)** — query chạy lại server-side và stream thẳng ra đĩa (kết quả lớn bao nhiêu cũng xuất được, không đi qua IPC), save dialog chọn nơi lưu, file dở dang tự xoá khi lỗi.
- Custom titlebar + cửa sổ; toàn bộ offline (không CDN).

## Kiến trúc

```
┌───────────────────────── apps/mieosql-desktop (Tauri 2) ─────────────────────────┐
│  React 19 + Vite + Tailwind 4 + CodeMirror 6     (frontend/, TS)                 │
│        ⇅ invoke() / events "mieosql://query"                                     │
│  commands.rs — 29 IPC commands                                                   │
├──────────────────────────────────────────────────────────────────────────────────┤
│  mieosql-api     DTO + Api facade + export-ts                                    │
│  mieosql-engine  sessions · vault · profile store · query history · cancel       │
│  mieosql-drivers AnyDriver (sqlx: SQLite, MySQL/MariaDB, PostgreSQL)             │
│  mieosql-core    value model · schema model · errors · profiles · query events   │
└──────────────────────────────────────────────────────────────────────────────────┘
        apps/mieosql-cli ← consumer thứ 2 của cùng một API
```

Chi tiết: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) · Đối chiếu Navicat: [docs/FEATURE_MATRIX.md](docs/FEATURE_MATRIX.md) · Lộ trình: [docs/ROADMAP.md](docs/ROADMAP.md)

## Bắt đầu nhanh

```bash
# Backend
cargo test                                   # integration test SQLite (không cần server)
./scripts/demo-sqlite.sh                     # demo end-to-end qua CLI

# Desktop app — binary LUÔN nhúng sẵn FE, không phụ thuộc dev server
cd apps/mieosql-desktop/frontend && npm install && npm run build
cargo build -p mieosql-desktop && ./target/debug/mieosql-desktop

# Kiểm tra render (in ra DOM trong webview thật):
MIEOSQL_SMOKE_CHECK=1 ./target/debug/mieosql-desktop

# Dev FE với hot-reload (HMR) — cần tauri-cli:
cargo install tauri-cli --version "^2"
cd apps/mieosql-desktop
cargo tauri dev --config tauri.dev.conf.json     # vite :5173 + app trỏ vào đó

# Đóng gói 1 file (.dmg / .AppImage / .exe) — tự build FE trước:
cd apps/mieosql-desktop && cargo tauri build
```

Khi sửa Rust DTO, sinh lại TS bindings (FE đọc từ `frontend/src/bindings`):

```bash
cargo run -p mieosql-api --bin export-ts -- apps/mieosql-desktop/frontend/src/bindings
```

## Layout

```
crates/mieosql-core       domain: values, schema, errors, profiles, query events (kèm TS derive)
crates/mieosql-drivers    AnyDriver: sqlx (SQLite/MySQL/MariaDB/PostgreSQL) + introspection
crates/mieosql-engine     Engine: sessions, vault, profile store, history
crates/mieosql-api        Api facade, DTO, export-ts binary
apps/mieosql-cli          CLI headless
apps/mieosql-desktop      Tauri 2 shell + React/TS frontend (frontend/src/bindings = TS contract)
docs/                     feature matrix (Navicat), architecture, roadmap
scripts/demo-sqlite.sh    demo end-to-end
```
