// Pure ER-diagram layout math: deterministic grid placement, per-table box
// sizing and foreign-key edge routing. No React / DOM dependencies so the
// whole module stays trivially testable.
//
// Coordinate space is "layout space": the calling panel renders an SVG of
// exactly `width` x `height` and applies pan/zoom with a wrapper transform.

import type { ForeignKeyDef, SchemaSnapshot, TableSnapshot } from "../lib/bindings";

export const BOX_WIDTH = 240;
export const BOX_HEADER_HEIGHT = 30;
export const COLUMN_ROW_HEIGHT = 18;
export const MORE_ROW_HEIGHT = 16;
export const BOX_PADDING_BOTTOM = 6;
export const GRID_GAP_X = 110;
export const GRID_GAP_Y = 80;
export const CANVAS_PADDING = 40;
/** Very wide tables are cut off after this many columns ("+N more"). */
export const MAX_COLUMNS_SHOWN = 24;
/** Keep the grid from becoming an unreadable single row on huge schemas. */
const GRID_MAX_COLUMNS = 10;

export type ErdColumnView = {
  name: string;
  type: string;
  pk: boolean;
  fk: boolean;
};

export type ErdBox = {
  table: TableSnapshot;
  x: number;
  y: number;
  width: number;
  height: number;
  columns: ErdColumnView[];
  hiddenColumns: number;
};

export type ErdEdge = {
  id: string;
  fk: ForeignKeyDef;
  from: { table: string; column: string };
  to: { table: string; column: string };
  /** Cubic-bezier path command in layout coordinates. */
  d: string;
  source: { x: number; y: number };
  target: { x: number; y: number };
  /** Degrees to rotate the arrowhead (drawn pointing right by default). */
  arrowAngle: number;
};

export type ErdStats = {
  tables: number;
  columns: number;
  foreignKeys: number;
  /** FKs whose referenced table is not part of the snapshot. */
  unresolved: number;
  truncated: boolean;
};

export type ErdLayout = {
  boxes: ErdBox[];
  edges: ErdEdge[];
  width: number;
  height: number;
  stats: ErdStats;
};

/** Y coordinate of the row center for `columnIndex` inside `box`. */
function columnCenterY(box: ErdBox, columnIndex: number): number {
  const shown = Math.max(1, box.columns.length);
  const idx = Math.min(Math.max(columnIndex, 0), shown - 1);
  return box.y + BOX_HEADER_HEIGHT + idx * COLUMN_ROW_HEIGHT + COLUMN_ROW_HEIGHT / 2;
}

/** Which vertical side of `box` faces `towardX`. */
function faceSide(box: ErdBox, towardX: number): "left" | "right" {
  return towardX >= box.x + box.width / 2 ? "right" : "left";
}

/**
 * Lay out every table of a snapshot on a grid and route FK edges between
 * column rows. Tables are ordered by FK degree (most connected first) so the
 * "hub" tables land in the top-left corner.
 */
export function computeErdLayout(snapshot: SchemaSnapshot): ErdLayout {
  const tables = snapshot.tables;

  // --- FK degree + per-table FK column lookup -----------------------------
  const degree = new Map<string, number>();
  for (const t of tables) degree.set(t.name, 0);

  const fkColumnsByTable = new Map<string, Set<string>>();
  for (const t of tables) {
    const cols = new Set<string>();
    for (const fk of t.foreign_keys) for (const c of fk.columns) cols.add(c);
    fkColumnsByTable.set(t.name, cols);
    for (const fk of t.foreign_keys) {
      degree.set(t.name, (degree.get(t.name) ?? 0) + 1);
      if (fk.ref_table !== t.name && degree.has(fk.ref_table)) {
        degree.set(fk.ref_table, (degree.get(fk.ref_table) ?? 0) + 1);
      }
    }
  }

  const ordered = [...tables].sort((a, b) => {
    const da = degree.get(a.name) ?? 0;
    const db = degree.get(b.name) ?? 0;
    if (da !== db) return db - da;
    return a.name.localeCompare(b.name);
  });

  const count = ordered.length;
  const gridColumns = Math.max(1, Math.min(GRID_MAX_COLUMNS, Math.ceil(Math.sqrt(count))));

  // --- Boxes: rows are as tall as their tallest member ---------------------
  const boxes: ErdBox[] = [];
  const boxesByName = new Map<string, ErdBox>();
  let rowTop = CANVAS_PADDING;

  for (let start = 0; start < count; start += gridColumns) {
    const rowTables = ordered.slice(start, start + gridColumns);
    const rowBoxes: ErdBox[] = [];
    rowTables.forEach((t, col) => {
      const fkCols = fkColumnsByTable.get(t.name) ?? new Set<string>();
      const shown = t.columns.slice(0, MAX_COLUMNS_SHOWN).map<ErdColumnView>((c) => ({
        name: c.name,
        type: c.native_type,
        pk: c.is_primary_key,
        fk: fkCols.has(c.name),
      }));
      const hiddenColumns = Math.max(0, t.columns.length - shown.length);
      const height =
        BOX_HEADER_HEIGHT +
        shown.length * COLUMN_ROW_HEIGHT +
        (hiddenColumns > 0 ? MORE_ROW_HEIGHT : 0) +
        BOX_PADDING_BOTTOM;
      const box: ErdBox = {
        table: t,
        x: CANVAS_PADDING + col * (BOX_WIDTH + GRID_GAP_X),
        y: rowTop,
        width: BOX_WIDTH,
        height,
        columns: shown,
        hiddenColumns,
      };
      rowBoxes.push(box);
      boxes.push(box);
      boxesByName.set(t.name, box);
    });
    rowTop += Math.max(...rowBoxes.map((b) => b.height)) + GRID_GAP_Y;
  }

  // --- Edges: cubic curves between the matching column rows ----------------
  const edges: ErdEdge[] = [];
  let unresolved = 0;

  for (const t of tables) {
    const sourceBox = boxesByName.get(t.name);
    if (!sourceBox) continue;
    t.foreign_keys.forEach((fk, i) => {
      const targetBox = boxesByName.get(fk.ref_table);
      if (!targetBox) {
        unresolved += 1;
        return;
      }
      const srcIndex = t.columns.findIndex((c) => fk.columns.includes(c.name));
      const targetColumn = fk.ref_columns[0] ?? t.columns[0]?.name ?? "";
      const targetIndex = targetBox.table.columns.findIndex((c) => c.name === targetColumn);
      const id = `${t.name}#${fk.name}#${i}`;
      const from = { table: t.name, column: fk.columns[0] ?? "" };
      const to = { table: fk.ref_table, column: targetColumn };

      if (sourceBox === targetBox) {
        // Self reference: loop out of (and back into) the right edge.
        const x = sourceBox.x + sourceBox.width;
        const sy = columnCenterY(sourceBox, srcIndex);
        const ty = columnCenterY(targetBox, targetIndex);
        const bulge = 80;
        edges.push({
          id,
          fk,
          from,
          to,
          d: `M ${x} ${sy} C ${x + bulge} ${sy}, ${x + bulge} ${ty}, ${x} ${ty}`,
          source: { x, y: sy },
          target: { x, y: ty },
          arrowAngle: 180,
        });
        return;
      }

      const sourceSide = faceSide(sourceBox, targetBox.x + targetBox.width / 2);
      const targetSide = faceSide(targetBox, sourceBox.x + sourceBox.width / 2);
      const sx = sourceSide === "right" ? sourceBox.x + sourceBox.width : sourceBox.x;
      const sy = columnCenterY(sourceBox, srcIndex);
      const tx = targetSide === "left" ? targetBox.x : targetBox.x + targetBox.width;
      const ty = columnCenterY(targetBox, targetIndex);
      const dir = sourceSide === "right" ? 1 : -1;
      const offset = Math.max(50, Math.abs(tx - sx) * 0.45);

      edges.push({
        id,
        fk,
        from,
        to,
        d: `M ${sx} ${sy} C ${sx + dir * offset} ${sy}, ${tx - dir * offset} ${ty}, ${tx} ${ty}`,
        source: { x: sx, y: sy },
        target: { x: tx, y: ty },
        arrowAngle: targetSide === "left" ? 0 : 180,
      });
    });
  }

  const width =
    boxes.length > 0
      ? Math.max(...boxes.map((b) => b.x + b.width)) + CANVAS_PADDING
      : CANVAS_PADDING * 2;
  const height =
    boxes.length > 0
      ? Math.max(...boxes.map((b) => b.y + b.height)) + CANVAS_PADDING
      : CANVAS_PADDING * 2;

  return {
    boxes,
    edges,
    width,
    height,
    stats: {
      tables: count,
      columns: tables.reduce((acc, t) => acc + t.columns.length, 0),
      foreignKeys: edges.length,
      unresolved,
      truncated: snapshot.truncated,
    },
  };
}
