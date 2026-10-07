# Architecture — MieoSQL Backend

## 1. Tổng quan

MieoSQL là app desktop đa engine (kiểu Navicat), đóng gói **một file thực thi duy nhất** mỗi OS,
viết bằng **Rust** (toàn bộ logic) + **TypeScript** (UI, sẽ ghép theo design riêng).

```
apps/mieosql-desktop   ← Tauri 2 shell: cửa sổ native, IPC commands/events, nhúng FE assets
apps/mieosql-cli       ← CLI headless: test BE không cần GUI, dùng cho CI
crates/mieosql-api     ← "hợp đồng" với FE: DTO + service façade + sinh TS bindings (ts-rs)
crates/mieosql-engine  ← phiên làm việc: connection manager, vault, query history, cancel
crates/mieosql-drivers ← hiện thực driver: sqlx (MySQL/MariaDB, PostgreSQL, SQLite)
crates/mieosql-core    ← domain thuần: value model, schema model, errors, profiles, events
```

Chiều phụ thuộc: `core ← drivers ← engine ← api ← apps`. **FE chỉ nói chuyện với `mieosql-api`.**

## 2. Vì sao tách crate như vậy?

| Tầng | Lý do tồn tại |
|---|---|
| `core` | Không phụ thuộc driver nào. Type có thể serialize, ổn định, dùng cho cả FE bindings. |
| `drivers` | Cô lập toàn bộ sqlx. Thêm engine (MSSQL, Mongo…) không đụng tầng trên. |
| `engine` | Quản lý state có vòng đời (session, secrets, history). Đây là "server" nội bộ của app. |
| `api` | Ranh giới IPC. Mọi thứ FE cần đều đi qua đây; đổi transport (Tauri IPC ⇄ HTTP/WS) không đổi DTO. |
| `apps` | Mỏng: chỉ wire runtime (Tauri window / CLI clap) vào façade. |

## 3. Luồng một câu query (streaming)

```
FE ──invoke(run_query)──▶ api.run_query(session, sql, params, options)
engine: cấp QueryId, spawn task, trả ngay (QueryId + event stream)
        │
        ▼
drivers: sqlx prepared/raw SQL trên connection của session
        │  mỗi batch N dòng (default 256)
        ▼
QueryEvent::Columns → QueryEvent::Batch(rows) × n → QueryEvent::Finished
        │
        ▼
Tauri emit "mieosql://query/{id}"  ──▶  FE append rows vào grid
```

- **Không bao giờ giữ toàn bộ kết quả trong RAM** — engine chỉ đẩy event theo batch.
- **Cancel**: mỗi query có `CancellationToken`; huỷ giữa 2 batch. (P1: hard-cancel phía server.)
- **Transaction**: session giữ `Transaction<'static, DB>`; khi có tx, mọi statement chạy trên tx
  đó thay vì pool — đúng ngữ nghĩa "một query tab = một connection" của Navicat.

## 4. Value model (cầu nối Rust ↔ TS)

`Value` là enum gắn tag, an toàn với JSON, **không mất độ chính xác**:

```ts
{ t: "int", v: 42 } | { t: "decimal", v: "123.4500" } | { t: "bytes", v: "<base64>" }
| { t: "datetime", v: "2026-10-07T12:00:00+07:00" } | { t: "json", v: {...} } | { t: "null" } | ...
```

- `Decimal` giữ dạng chuỗi — tránh sai số float khi hiển thị tiền tệ.
- `Bytes` → base64 (BLOB/binary hiển thị hex ở FE, xem raw view ở banner).
- `Unsupported` giữ `native_type` để FE hiển thị `‹type không đọc được›` thay vì crash.
- `DateTime` luôn kèm offset khi engine có timezone (TIMESTAMPTZ / my TIMESTAMP).

## 5. Driver abstraction

Một engine mới = **một variant** trong `AnyDriver` + hiện thực các method introspection
(`list_databases`, `list_objects`, `columns`, `indexes`, `foreign_keys`, `ddl`…), query streaming,
transaction. Dispatch bằng macro `dispatch!(self, d => d.method(...))` — zero chi phí dynamic.

`Capabilities` cho FE biết engine làm được gì (list databases? schemas? placeholder style? tx? simple mode?)
để UI ẩn/hiện đúng tính năng.

## 6. Bảo mật credentials

- `profiles.json`: chỉ metadata (host, user, tên…), **không chứa password**.
- `secrets.json`: từng secret mã hoá **XChaCha20-Poly1305**; master key 32 byte random trong `master.key`
  (quyền `0600`), nằm trong app-data dir của OS.
- P2: chuyển master key vào OS Keychain (macOS Keychain / Windows Credential Manager / libsecret).
- Không log secret; `ConnectionSpec` debug-in bằng `password: <redacted>`.

## 7. Thêm engine mới (checklist)

1. Thêm biến thể vào `Engine` (core) — FE tự nhận vì bindings sinh từ Rust.
2. Thêm variant vào `AnyDriver` + arm dispatch.
3. Hiện thực introspection + query cho engine (SQL catalog riêng của engine đó).
4. Khai báo `Capabilities`.
5. Viết integration test (SQLite chạy được không cần server; engine khác dùng container nếu CI hỗ trợ).

## 8. Đóng gói "một file chạy duy nhất"

- Tauri nhúng FE (HTML/JS/CSS đã build) vào binary → **1 file** `.app`/`.dmg` (macOS), `.AppImage` (Linux),
  `.exe`/NSIS (Windows).
- Không phụ thuộc runtime ngoài: sqlx thuần Rust, TLS = rustls (không OpenSSL), SQLite bunded (không libsystem).
- `cargo tauri build` — cùng một codebase, 3 nền tảng; macOS cần ký + notarize khi phát hành (P1).

## 9. Testing

- Unit: value conversion, error mapping, vault round-trip, profile store.
- Integration SQLite: dựng DB tạm → tạo bảng → introspection → query streaming → cancel → transaction.
- MySQL/Postgres: test đánh dấu `#[ignore]`, chạy khi có `MIEOSQL_TEST_MYSQL_URL` / `MIEOSQL_TEST_PG_URL`.
- CLI là "smoke test" thủ công: `mieosql-cli query ...` phải chạy được trên cả 3 engine.

## 10. Nguyên tắc code

- `core` không có `async` (thuần data), `drivers/engine` async bằng tokio.
- Lỗi có cấu trúc: `ErrorInfo { code, message, detail, hint, position, db_code }` — FE map thẳng ra UI.
- Mọi API surface của FE **có version** (`API_VERSION`) để FE cũ không vỡ khi BE đổi.
- Không panic vì dữ liệu từ server DB; decode thất bại ⇒ `Unsupported`, không `unwrap`.

## 11. Frontend (v0.2)

Stack: **React 19 + Vite 8 + Tailwind 4 + CodeMirror 6 + zustand**, thuần offline (không CDN).
Nguồn: `apps/mieosql-desktop/frontend/`, bindings ở `frontend/src/bindings` (sinh bởi `export-ts`,
commit vào repo, CI diff kiểm tra lệch).

- **Data flow**: `listen("mieosql://query")` → `store.handleQueryEvent` → result sets trong tab.
  Run/Search/Preview đi qua `invoke(...)`; mọi thứ typed bằng bindings.
- **Excel-like grid** (`ResultGrid`): div-based (không `<table>`), header sticky với type badge,
  resize/auto-fit cột, sort client-side, filter client-side (deferred), **virtualization cố định
  22px/dòng** chỉ render vùng nhìn thấy + overscan. Copy cell/row, cell viewer cho JSON/base64.
- **SQL editor**: CodeMirror với dialect theo engine; text lưu vào store debounce 350ms + khi
  đổi tab; Ctrl+Enter chạy; Ln/Col về status bar.
- **Cây connections**: load lazy theo node, declarative (expand + cache → effect nạp), PG tự
  `use_database` (reconnect) khi mở database khác; search lọc client-side và truyền pattern
  khi nạp folder.
- **Tauri**: custom titlebar (`decorations: false`), capabilities tối thiểu
  (`core:default` + window minimize/maximize/close/start-dragging).
- **Precision**: `BIGINT`/`UINT` trong `Value` serialize dạng **string**; counter metadata
  (`rows`, `elapsed_ms`, `batch_index`, …) là `number` (ts-rs override `#[ts(type = "number")]`).
