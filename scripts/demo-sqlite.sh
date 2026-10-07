#!/usr/bin/env bash
# Demo end-to-end của backend MieoSQL qua CLI + SQLite (không cần server).
set -euo pipefail

DIR="$(mktemp -d "${TMPDIR:-/tmp}/mieosql-demo.XXXXXX")"
DB="$DIR/demo.db"
DATA="$DIR/appdata"
touch "$DB"

BIN=(cargo run -q -p mieosql-cli -- --data-dir "$DATA")

echo "== demo dir: $DIR"
echo
echo "== 1. thêm connection profile"
"${BIN[@]}" profile add --name demo --engine sqlite --file "$DB" --ssl-mode disable

echo
echo "== 2. test connection"
"${BIN[@]}" test demo

echo
echo "== 3. tạo schema"
"${BIN[@]}" query demo "CREATE TABLE products (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  name TEXT NOT NULL,
  price NUMERIC,
  tags TEXT,
  created_at TEXT DEFAULT (datetime('now'))
)"

echo
echo "== 4. insert với tham số"
"${BIN[@]}" query demo "INSERT INTO products (name, price, tags) VALUES
  ('Espresso Machine', 1299.99, 'kitchen,coffee'),
  ('Grinder', 249.50, 'kitchen'),
  ('Scale', NULL, 'measure')"

echo
echo "== 5. select (bảng)"
"${BIN[@]}" query demo "SELECT id, name, price, tags FROM products ORDER BY id"

echo
echo "== 6. select (json)"
"${BIN[@]}" query demo "SELECT name, price FROM products ORDER BY price DESC" --format json

echo
echo "== 7. introspection: schema"
"${BIN[@]}" schema demo

echo
echo "== 8. introspection: columns + indexes"
"${BIN[@]}" columns demo products

echo
echo "== 9. DDL"
"${BIN[@]}" ddl demo products

echo
echo "== 10. lịch sử query"
"${BIN[@]}" history --limit 5

echo
echo "demo hoàn tất. app data (profiles/vault/history): $DATA"
